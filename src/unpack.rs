use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, Result};
use async_compression::tokio::bufread::ZstdDecoder;
use futures::{
    stream::{self, StreamExt},
    TryFutureExt, TryStreamExt,
};
use fxhash::FxHashMap;
use rattler::package_cache::{CacheKey, PackageCache};
use rattler_conda_types::{PackageRecord, Platform, RepoData, RepoDataRecord};
use rattler_package_streaming::fs::extract;
use rattler_shell::{
    activation::{ActivationVariables, Activator, PathModificationBehavior},
    shell::{Shell, ShellEnum},
};
use tokio::fs::{self, create_dir_all};
use tokio_stream::wrappers::ReadDirStream;
use tokio_tar::Archive;
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
const HISTORY_FILE: &str = "history";

/// Unpack a pixi environment.
pub async fn unpack(options: UnpackOptions) -> Result<()> {
    // TODO: Dont use static dir here but a temp dir
    let unpack_dir = Arc::from(options.output_directory.join("unpack"));
    create_dir_all(&unpack_dir)
        .await
        .map_err(|e| anyhow!("Could not create unpack directory: {}", e))?;

    let cache_dir = Path::new(CACHE_DIR);
    create_dir_all(cache_dir)
        .await
        .map_err(|e| anyhow!("Could not create cache directory: {}", e))?;

    unarchive(&options.pack_file, &unpack_dir)
        .await
        .map_err(|e| anyhow!("Could not unarchive: {}", e))?;

    // Read pixi-pack.json metadata file
    let metadata_file = unpack_dir.join("pixi-pack.json");

    let metadata_contents = tokio::fs::read_to_string(&metadata_file)
        .await
        .map_err(|e| anyhow!("Could not read metadata file: {}", e))?;

    let metadata: PixiPackMetadata = serde_json::from_str(&metadata_contents)?;

    if metadata.version != DEFAULT_PIXI_PACK_VERSION {
        anyhow::bail!("Unsupported pixi-pack version: {}", metadata.version);
    }
    if metadata.platform != Platform::current() {
        anyhow::bail!("The pack was created for a different platform");
    }

    let channel = unpack_dir.join(CHANNEL_DIRECTORY_NAME);
    let packages = collect_packages(&channel)
        .await
        .map_err(|e| anyhow!("could not collect packages: {}", e))?;

    // extract packages to cache
    let package_cache = PackageCache::new(cache_dir);

    let installer = rattler::install::Installer::default();

    let prefix = options.output_directory.join("env");

    let repodata_records: Vec<RepoDataRecord> = stream::iter(packages)
        .map(|(file_name, package_record)| {
            let cache_key = CacheKey::from(&package_record);

            let package_path = channel.join(&package_record.subdir).join(&file_name);

            let repodata_record = RepoDataRecord {
                package_record,
                file_name,
                url: Url::parse("http://nonexistent").unwrap(),
                channel: "local".to_string(),
            };

            async {
                // We have to prepare the package cache by inserting all packages into it.
                // We can only do so by calling `get_or_fetch` on each package, which will
                // use the provided closure to fetch the package and insert it into the cache.
                package_cache
                    .get_or_fetch(
                        cache_key,
                        move |destination| {
                            async move { extract(&package_path, &destination).map(|_| ()) }
                        },
                        None,
                    )
                    .await
                    .map_err(|e| anyhow!("could not extract package: {}", e))?;

                Ok::<RepoDataRecord, anyhow::Error>(repodata_record)
            }
        })
        .buffer_unordered(50)
        .try_collect()
        .await?;

    // Invariant: all packages are in the cache

    installer
        .with_package_cache(package_cache)
        .install(&prefix, repodata_records)
        .await
        .map_err(|e| anyhow!("could not install packages: {}", e))?;

    let history_path = prefix.join("conda-meta").join(HISTORY_FILE);

    fs::write(
        history_path,
        "// not relevant for pixi but for `conda run -p`",
    )
    .map_err(|e| anyhow!("Could not write history file: {}", e))
    .await?;

    tracing::debug!("Cleaning up unpack directory");
    fs::remove_dir_all(unpack_dir)
        .await
        .map_err(|e| anyhow!("Could not remove unpack directory: {}", e))?;

    let shell = options.shell.unwrap_or_default();
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
    fs::write(activate_path, contents)
        .map_err(|e| anyhow!("Could not write activate script: {}", e))
        .await?;

    Ok(())
}

async fn collect_packages_in_subdir(subdir: PathBuf) -> Result<FxHashMap<String, PackageRecord>> {
    let repodata = subdir.join("repodata.json");

    let raw_repodata_json = fs::read_to_string(repodata)
        .await
        .map_err(|e| anyhow!("could not read repodata in subdir: {}", e))?;

    let repodata: RepoData = serde_json::from_str(&raw_repodata_json).map_err(|e| {
        anyhow!(
            "could not parse repodata in subdir {}: {}",
            subdir.display(),
            e
        )
    })?;

    let mut conda_packages = repodata.conda_packages;
    let packages = repodata.packages;
    conda_packages.extend(packages);
    Ok(conda_packages)
}

/* -------------------------------------- INSTALL PACKAGES ------------------------------------- */

/// Collect all packages in a directory.
async fn collect_packages(channel_dir: &Path) -> Result<FxHashMap<String, PackageRecord>> {
    let subdirs = fs::read_dir(channel_dir)
        .await
        .map_err(|e| anyhow!("could not read channel directory: {}", e))?;

    let stream = ReadDirStream::new(subdirs);

    let packages = stream
        .try_filter_map(|entry| async move {
            let subdir = entry;
            if subdir.path().is_dir() {
                Ok(Some(collect_packages_in_subdir(subdir.path())))
            } else {
                Ok(None) // Ignore non-directory entries
            }
        })
        .map_err(|e| anyhow!("could not read channel directory: {}", e))
        .try_buffer_unordered(10)
        .try_fold(FxHashMap::default(), |mut acc, packages| async move {
            acc.extend(packages);
            Ok(acc)
        })
        .await?;

    Ok(packages)
}

/* ----------------------------------- UNARCHIVE + DECOMPRESS ---------------------------------- */

/// Unarchive a compressed tarball.
async fn unarchive(archive_path: &Path, target_dir: &Path) -> Result<()> {
    let file = fs::File::open(archive_path)
        .await
        .map_err(|e| anyhow!("could not open archive {:#?}: {}", archive_path, e))?;

    let reader = tokio::io::BufReader::new(file);

    let decocder = ZstdDecoder::new(reader);

    let mut archive = Archive::new(decocder);

    archive
        .unpack(target_dir)
        .await
        .map_err(|e| anyhow!("could not unpack archive: {}", e))?;

    Ok(())
}
