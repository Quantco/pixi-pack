use core::fmt;
use std::{collections::HashMap, path::{Path, PathBuf}, sync::Arc};

use futures::future::try_join_all;
use rattler::{install::{link_package, InstallDriver, InstallOptions}, package_cache::{CacheKey, PackageCache}};
use rattler_conda_types::{PackageRecord, Platform, PrefixRecord, RepoData, RepoDataRecord};
use rattler_package_streaming::{fs::extract, ExtractError};
use rattler_shell::{activation::{ActivationVariables, Activator, PathModificationBehavior}, shell::{Shell, ShellEnum}};
use url::Url;

use crate::{PixiPackMetadata, DEFAULT_PIXI_PACK_VERSION, CHANNEL_DIRECTORY_NAME};

/* ------------------------------------------- UNPACK ------------------------------------------ */

/// Options for unpacking a pixi environment.
#[derive(Debug)]
pub struct UnpackOptions {
    pub pack_file: PathBuf,
    pub output_directory: PathBuf,
    pub shell: Option<ShellEnum>,
}

const CACHE_DIR: &str = "cache";
const HISTORY_FILE: &str = "history";

#[derive(Debug)]
enum UnpackError { 
    ExtractError(ExtractError),
}

impl fmt::Display for UnpackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnpackError::ExtractError(e) => write!(f, "An error occurred while extracting the package: {}", e),
        }
    }
}

impl std::error::Error for UnpackError { }

impl From<ExtractError> for UnpackError {
    fn from(e: ExtractError) -> Self {
        UnpackError::ExtractError(e)
    }
}


/// Unpack a pixi environment.
pub async fn unpack(options: UnpackOptions) -> Result<(), Box<dyn std::error::Error>> {
    let unpack_dir = Arc::from(options.output_directory.join("unpack"));
    std::fs::create_dir_all(&unpack_dir).expect("Could not create unpack directory");
    let cache_dir = Path::new(CACHE_DIR);
    std::fs::create_dir_all(&cache_dir).expect("Could not create cache directory");
    unarchive(&options.pack_file, &unpack_dir);

    // Read pixi-pack.json metadata file
    let metadata_file = unpack_dir.join("pixi-pack.json");
    let metadata_contents = std::fs::read_to_string(&metadata_file).expect("Could not read metadata file");
    let metadata: PixiPackMetadata = serde_json::from_str(&metadata_contents)?;
    if metadata.version != DEFAULT_PIXI_PACK_VERSION {
        panic!("Unsupported pixi-pack version: {}", metadata.version);
    }
    if metadata.platform != Platform::current() {
        panic!("The pack was created for a different platform");
    }

    let channel = unpack_dir.join(CHANNEL_DIRECTORY_NAME);
    let packages = collect_packages(&channel).unwrap();

    // extract packages to cache
    let package_cache = PackageCache::new(cache_dir);

    let mut repodata_records = vec![];
    for (filename, package_record) in &packages {
        let repodata_record = RepoDataRecord {
            package_record: package_record.clone(),
            file_name: filename.clone(),
            url: Url::parse("http://nonexistent").unwrap(),
            channel: "local".to_string()
        };
        repodata_records.push(repodata_record);
    }
    let mut iter = vec![];
    for (filename, pkg_record) in packages {
        let cache_key = CacheKey::from(&pkg_record);
        let channel = channel.clone();
        let result = package_cache.get_or_fetch(cache_key, |destination| async move {
            let package_path = channel.join(pkg_record.subdir).join(filename);
            extract(&package_path, &destination)?;
            Ok::<(), UnpackError>(())
        }, None);
        iter.push(result);
    }
    let extracted_packages = try_join_all(iter).await?;
    let prefix = options.output_directory.join("env");

    let install_driver = InstallDriver::default();
    let install_options = InstallOptions::default();
    let mut iter = vec![];
    for (package_path, repodata_record) in extracted_packages.iter().zip(repodata_records) {
        // install packages from cache
        let result = install_package_to_environment(
            &prefix,
            package_path.clone(),
            repodata_record,
            &install_driver,
            &install_options,
        );
        iter.push(result);
    }
    try_join_all(iter).await?;

    let history_path = prefix.join("conda-meta").join(HISTORY_FILE);
    std::fs::write(history_path, "// not relevant for pixi but for `conda run -p`").expect("Could not write history file");

    tracing::debug!("Cleaning up unpack directory");
    std::fs::remove_dir_all(unpack_dir).expect("Could not remove unpack directory");

    let shell = match options.shell {
        Some(shell) => shell,
        None => ShellEnum::default(),
    };
    let file_extension = shell.extension();
    let activate_path = options.output_directory.join(format!("activate.{}", file_extension));
    let activator = Activator::from_path(prefix.as_path(), shell, Platform::current())?;
    
    let path = std::env::var("PATH")
    .ok()
    .map(|p| std::env::split_paths(&p).collect::<Vec<_>>());

    // If we are in a conda environment, we need to deactivate it before activating the host / build prefix
    let conda_prefix = std::env::var("CONDA_PREFIX").ok().map(|p| p.into());
    let result = activator
        .activation(ActivationVariables {
            conda_prefix,
            path,
            path_modification_behavior: PathModificationBehavior::default(),
        })?;

    let contents = result.script.contents()?;
    std::fs::write(activate_path, contents).unwrap();

    Ok(())
}

/* -------------------------------------- INSTALL PACKAGES ------------------------------------- */

/// Collect all packages in a directory.
fn collect_packages(channel: &Path) -> Result<HashMap<String, PackageRecord>, Box<dyn std::error::Error>> {
    let subdirs = channel.read_dir()?;
    let packages = subdirs
        .into_iter()
        .filter(|subdir| subdir.as_ref().is_ok_and(|subdir| subdir.path().is_dir()))
        .flat_map(|subdir| {
            let subdir = subdir.unwrap().path();
            let repodata = subdir.join("repodata.json");
            let repodata = RepoData::from_path(repodata).unwrap();
            let mut conda_packages = repodata.conda_packages;
            let packages = repodata.packages;
            conda_packages.extend(packages.into_iter());
            conda_packages
        })
        .collect();
    Ok(packages)
}

/// Install a package into the environment and write a `conda-meta` file that contains information
/// about how the file was linked.
async fn install_package_to_environment(
    target_prefix: &Path,
    package_dir: PathBuf,
    repodata_record: RepoDataRecord,
    install_driver: &InstallDriver,
    install_options: &InstallOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    // Link the contents of the package into our environment. This returns all the paths that were
    // linked.
    let paths = link_package(
        &package_dir,
        target_prefix,
        install_driver,
        install_options.clone(),
    )
    .await?;

    // Construct a PrefixRecord for the package
    let prefix_record = PrefixRecord {
        repodata_record,
        package_tarball_full_path: None,
        extracted_package_dir: Some(package_dir),
        files: paths
            .iter()
            .map(|entry| entry.relative_path.clone())
            .collect(),
        paths_data: paths.into(),
        requested_spec: None,
        link: None,
    };

    // Create the conda-meta directory if it doesn't exist yet.
    let target_prefix = target_prefix.to_path_buf();
    match tokio::task::spawn_blocking(move || {
        let conda_meta_path = target_prefix.join("conda-meta");
        std::fs::create_dir_all(&conda_meta_path)?;

        // Write the conda-meta information
        let pkg_meta_path = conda_meta_path.join(format!(
            "{}-{}-{}.json",
            prefix_record
                .repodata_record
                .package_record
                .name
                .as_normalized(),
            prefix_record.repodata_record.package_record.version,
            prefix_record.repodata_record.package_record.build
        ));
        prefix_record.write_to_path(pkg_meta_path, true)
    })
    .await
    {
        Ok(result) => Ok(result?),
        Err(err) => {
            if let Ok(panic) = err.try_into_panic() {
                std::panic::resume_unwind(panic);
            }
            // The operation has been cancelled, so we can also just ignore everything.
            Ok(())
        }
    }
}

/* ----------------------------------- UNARCHIVE + DECOMPRESS ---------------------------------- */

/// Unarchive a compressed tarball.
fn unarchive(archive_path: &Path, target_dir: &Path) {
    let file = std::fs::File::open(&archive_path).expect("could not open archive");
    let decoder = zstd::Decoder::new(file).expect("could not instantiate zstd decoder");
    tar::Archive::new(decoder)
        .unpack(target_dir)
        .expect("could not unpack archive")
}
