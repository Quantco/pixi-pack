use core::fmt;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use tempdir::TempDir;

use futures::stream::{self, StreamExt};
use rattler::package_cache::{CacheKey, PackageCache};
use rattler_conda_types::{PackageRecord, Platform, RepoData, RepoDataRecord};
use rattler_package_streaming::{fs::extract, ExtractError};
use rattler_shell::{
    activation::{ActivationVariables, Activator, PathModificationBehavior},
    shell::{Shell, ShellEnum},
};
use url::Url;

use crate::{PixiPackMetadata, CHANNEL_DIRECTORY_NAME, DEFAULT_PIXI_PACK_VERSION};

/* ------------------------------------------- UNPACK ------------------------------------------ */

/// Options for unpacking a pixi environment.
#[derive(Debug)]
pub struct UnpackOptions {
    pub pack_file: PathBuf,
    pub output_directory: PathBuf,
    pub shell: Option<ShellEnum>,
}

const CACHE_DIR: &str = "cache";
const ENV_DIR: &str = "env";

#[derive(Debug)]
enum UnpackError {
    ExtractError(ExtractError),
}

impl fmt::Display for UnpackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnpackError::ExtractError(e) => {
                write!(f, "An error occurred while extracting the package: {}", e)
            }
        }
    }
}

impl std::error::Error for UnpackError {}

impl From<ExtractError> for UnpackError {
    fn from(e: ExtractError) -> Self {
        UnpackError::ExtractError(e)
    }
}

/// Unpack a pixi environment.
pub async fn unpack(options: UnpackOptions) -> crate::Result<()> {
    // unarchive the pack file
    let unpack_dir = TempDir::new("pixi-pack-unpack")
        .expect("Could not create temporary unpack directory")
        .into_path();
    let cache_dir = Path::new(CACHE_DIR);
    std::fs::create_dir_all(cache_dir)?;
    tracing::debug!(
        "Unpacking {} to {}",
        options.pack_file.display(),
        unpack_dir.display()
    );
    unarchive(&options.pack_file, &unpack_dir)?;

    // Read pixi-pack.json metadata file
    let metadata_file = unpack_dir.join("pixi-pack.json");
    let metadata_contents = std::fs::read_to_string(&metadata_file)?;
    let metadata: PixiPackMetadata = serde_json::from_str(&metadata_contents)?;
    if metadata.version != DEFAULT_PIXI_PACK_VERSION {
        panic!("Unsupported pixi-pack version: {}", metadata.version);
    }
    if metadata.platform != Platform::current() {
        panic!("The pack was created for a different platform");
    }

    // collect packages from pack
    let channel = unpack_dir.join(CHANNEL_DIRECTORY_NAME);
    let packages = collect_packages(&channel)?;

    // extract packages to cache
    let package_cache = PackageCache::new(cache_dir);
    let iter = packages.into_iter().map(|(filename, pkg_record)| async {
        let cache_key = CacheKey::from(&pkg_record);
        let channel = channel.clone();

        let repodata_record = RepoDataRecord {
            package_record: pkg_record.clone(),
            file_name: filename.clone(),
            url: Url::parse("http://nonexistent").unwrap(),
            channel: "local".to_string(),
        };

        package_cache
            .get_or_fetch(
                cache_key,
                move |destination| {
                    let package_path = channel.join(pkg_record.subdir).join(filename);
                    extract(&package_path, &destination).expect(
                        format!(
                            "Could not extract package {} to {}",
                            package_path.display(),
                            destination.display()
                        )
                        .as_str(),
                    );
                    async { Ok::<(), UnpackError>(()) }
                },
                None,
            )
            .await
            .unwrap(); // error was already handled in extract part
        repodata_record
    });

    let installer = rattler::install::Installer::default();
    let prefix = options.output_directory.join(ENV_DIR);
    let repodata_records = stream::iter(iter)
        .buffer_unordered(50)
        .collect::<Vec<_>>()
        .await;
    // This uses the side-effect that the package cache is populated from before with all our packages.
    // Thus, no need to fetch anything from the internet here.
    installer
        .with_package_cache(package_cache)
        .install(&prefix, repodata_records)
        .await?;

    let history_path = prefix.join("conda-meta").join("history");
    std::fs::write(
        history_path,
        "// not relevant for pixi but for `conda run -p`",
    )?;

    tracing::debug!("Cleaning up unpack directory {}", unpack_dir.display());
    std::fs::remove_dir_all(unpack_dir)?;

    tracing::debug!("Creating activation script");
    let shell = match options.shell {
        Some(shell) => shell,
        None => ShellEnum::default(),
    };
    let file_extension = shell.extension();
    let activate_path = options
        .output_directory
        .join(format!("activate.{}", file_extension));
    let activator = Activator::from_path(prefix.as_path(), shell, Platform::current())?;

    let path = std::env::var("PATH")
        .ok()
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>());

    // If we are in a conda environment, we need to deactivate it before activating the host / build prefix
    let conda_prefix = std::env::var("CONDA_PREFIX").ok().map(|p| p.into());
    let result = activator.activation(ActivationVariables {
        conda_prefix,
        path,
        path_modification_behavior: PathModificationBehavior::default(),
    })?;

    let contents = result.script.contents()?;
    std::fs::write(activate_path, contents)?;
    Ok(())
}

/* -------------------------------------- INSTALL PACKAGES ------------------------------------- */

/// Collect all packages in a directory.
fn collect_packages(channel: &Path) -> crate::Result<HashMap<String, PackageRecord>> {
    let subdirs = channel.read_dir()?;
    let packages = subdirs
        .into_iter()
        .map(|subdir| subdir.expect("todo error handling"))
        .filter(|subdir| subdir.path().is_dir())
        .flat_map(|subdir| {
            let repodata = subdir.path().join("repodata.json");
            let repodata = RepoData::from_path(repodata).expect("TODO error handling");
            let mut conda_packages = repodata.conda_packages;
            let packages = repodata.packages;
            conda_packages.extend(packages);
            conda_packages
        })
        .collect::<HashMap<_, _>>();
    Ok(packages)
}

/* ----------------------------------- UNARCHIVE + DECOMPRESS ---------------------------------- */

/// Unarchive a compressed tarball.
fn unarchive(archive_path: &Path, target_dir: &Path) -> crate::Result<()> {
    let file = std::fs::File::open(archive_path)?;
    let decoder = zstd::Decoder::new(file).expect("Could not instantiate zstd decoder");
    tar::Archive::new(decoder).unpack(target_dir)?;
    Ok(())
}
