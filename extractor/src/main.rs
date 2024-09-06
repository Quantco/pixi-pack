use std::path::{Path, PathBuf};

use fxhash::FxHashMap;
use tokio::fs;
use tokio_stream::wrappers::ReadDirStream;

use anyhow::anyhow;
use anyhow::Result;
use futures::{stream, StreamExt, TryFutureExt, TryStreamExt};
use rattler::{
    install::Installer,
    package_cache::{CacheKey, PackageCache},
};
use rattler_conda_types::{PackageRecord, Platform, RepoData, RepoDataRecord};
use rattler_package_streaming::fs::extract;
use rattler_shell::{
    activation::{ActivationVariables, Activator, PathModificationBehavior},
    shell::{Shell, ShellEnum},
};
use serde::{Deserialize, Serialize};
use url::Url;

/// The metadata for a "pixi-pack".
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct PixiPackMetadata {
    /// The pack format version.
    pub version: String,
    /// The platform the pack was created for.
    pub platform: Platform,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        return Err(anyhow!(
            "Usage: {} <task> <input_dir> <output_dir>",
            args[0]
        ));
    }

    let task = &args[1];
    let input_dir = Path::new(&args[2]);
    let output_dir = Path::new(&args[3]);

    if task == "unpack" {
        unpack(input_dir, output_dir).await?;
    } else if task == "create-script" {
        create_activation_script(input_dir, output_dir).await?;
    } else {
        return Err(anyhow!(
            "Unknown task: {}. Task should be either 'unpack' or 'create-script'",
            task
        ));
    }

    Ok(())
}

/// Unpack a pixi environment from a directory
pub async fn unpack(archive_dir: &Path, output_dir: &Path) -> Result<()> {
    let channel_directory = archive_dir.join(std::env::var("PIXI_PACK_CHANNEL_DIRECTORY").unwrap());
    let cache_dir = archive_dir.join("cache");

    validate_metadata_file(archive_dir.join(std::env::var("PIXI_PACK_METADATA_PATH").unwrap()))
        .await?;

    create_prefix(&channel_directory, output_dir, &cache_dir)
        .await
        .map_err(|e| anyhow!("Could not create prefix: {}", e))?;

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

async fn validate_metadata_file(metadata_file: PathBuf) -> Result<()> {
    let metadata_contents = fs::read_to_string(&metadata_file)
        .await
        .map_err(|e| anyhow!("Could not read metadata file: {}", e))?;

    let metadata: PixiPackMetadata = serde_json::from_str(&metadata_contents)?;

    if metadata.version != std::env::var("PIXI_PACK_DEFAULT_VERSION").unwrap() {
        anyhow::bail!("Unsupported pixi-pack version: {}", metadata.version);
    }
    if metadata.platform != Platform::current() {
        anyhow::bail!("The pack was created for a different platform");
    }

    Ok(())
}

/// Collect all packages in a directory.
async fn collect_packages(channel_dir: &Path) -> Result<FxHashMap<String, PackageRecord>> {
    let subdirs = fs::read_dir(channel_dir)
        .await
        .map_err(|e| anyhow!("could not read channel directory: {}", e))?;

    let stream = ReadDirStream::new(subdirs);

    let packages = stream
        .try_filter_map(|entry| async move {
            let path = entry.path();

            if path.is_dir() {
                Ok(Some(path))
            } else {
                Ok(None) // Ignore non-directory entries
            }
        })
        .map_ok(collect_packages_in_subdir)
        .map_err(|e| anyhow!("could not read channel directory: {}", e))
        .try_buffer_unordered(10)
        .try_concat()
        .await?;

    Ok(packages)
}

async fn create_prefix(channel_dir: &Path, target_prefix: &Path, cache_dir: &Path) -> Result<()> {
    let packages = collect_packages(channel_dir)
        .await
        .map_err(|e| anyhow!("could not collect packages: {}", e))?;

    eprintln!(
        "⏳ Extracting and installing {} packages to {}...",
        packages.len(),
        cache_dir.display()
    );
    // extract packages to cache
    let package_cache = PackageCache::new(cache_dir);

    let repodata_records: Vec<RepoDataRecord> = stream::iter(packages)
        .map(|(file_name, package_record)| {
            let cache_key = CacheKey::from(&package_record);

            let package_path = channel_dir.join(&package_record.subdir).join(&file_name);

            let url = Url::parse(&format!("file:///{}", file_name)).unwrap();

            let repodata_record = RepoDataRecord {
                package_record,
                file_name,
                url,
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
                            let package_path_clone = package_path.clone();
                            async move { extract(&package_path_clone, &destination).map(|_| ()) }
                        },
                        None,
                    )
                    .await
                    .map_err(|e| {
                        anyhow!(
                            "could not extract \"{}\": {}",
                            repodata_record.as_ref().name.as_source(),
                            e
                        )
                    })?;

                Ok::<RepoDataRecord, anyhow::Error>(repodata_record)
            }
        })
        .buffer_unordered(50)
        .try_collect()
        .await?;

    // Invariant: all packages are in the cache
    let installer = Installer::default();
    installer
        .with_package_cache(package_cache)
        .install(&target_prefix, repodata_records)
        .await
        .map_err(|e| anyhow!("could not install packages: {}", e))?;

    let history_path = target_prefix.join("conda-meta").join("history");

    fs::write(
        history_path,
        "// not relevant for pixi but for `conda run -p`",
    )
    .map_err(|e| anyhow!("Could not write history file: {}", e))
    .await?;

    Ok(())
}

async fn create_activation_script(destination: &Path, prefix: &Path) -> Result<()> {
    let shell = ShellEnum::default();
    let file_extension = shell.extension();
    let activate_path = destination.join(format!("activate.{}", file_extension));
    let activator = Activator::from_path(prefix, shell, Platform::current())?;

    let result = activator.activation(ActivationVariables {
        conda_prefix: None,
        path: None,
        path_modification_behavior: PathModificationBehavior::Prepend,
    })?;

    let contents = result.script.contents()?;
    fs::write(activate_path, contents)
        .await
        .map_err(|e| anyhow!("Could not write activate script: {}", e))?;

    Ok(())
}

/* --------------------------------------------------------------------------------------------- */
/*                                             TESTS                                             */
/* --------------------------------------------------------------------------------------------- */
#[cfg(test)]
mod tests {
    use super::*;
    use rstest::*;
    use serde_json::json;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn other_platform() -> Platform {
        match Platform::current() {
            Platform::Linux64 => Platform::Win64,
            _ => Platform::Linux64,
        }
    }

    #[fixture]
    fn metadata_file(
        #[default(std::env::var("PIXI_PACK_DEFAULT_VERSION").unwrap())] version: String,
        #[default(Platform::current())] platform: Platform,
    ) -> NamedTempFile {
        let mut metadata_file = NamedTempFile::new().unwrap();
        let metadata = PixiPackMetadata { version, platform };
        let buffer = metadata_file.as_file_mut();
        buffer
            .write_all(json!(metadata).to_string().as_bytes())
            .unwrap();
        metadata_file
    }

    #[rstest]
    #[tokio::test]
    async fn test_metadata_file_valid(metadata_file: NamedTempFile) {
        assert!(validate_metadata_file(metadata_file.path().to_path_buf())
            .await
            .is_ok())
    }

    #[rstest]
    #[tokio::test]
    async fn test_metadata_file_empty() {
        assert!(
            validate_metadata_file(NamedTempFile::new().unwrap().path().to_path_buf())
                .await
                .is_err()
        )
    }

    #[rstest]
    #[tokio::test]
    async fn test_metadata_file_non_existent() {
        assert!(validate_metadata_file(PathBuf::new()).await.is_err())
    }

    #[rstest]
    #[tokio::test]
    async fn test_metadata_file_invalid_version(
        #[with("v0".to_string())] metadata_file: NamedTempFile,
    ) {
        let result = validate_metadata_file(metadata_file.path().to_path_buf()).await;
        let error = result.unwrap_err();
        assert_eq!(error.to_string(), "Unsupported pixi-pack version: v0");
    }

    #[rstest]
    #[tokio::test]
    async fn test_metadata_file_wrong_platform(
        #[with(std::env::var("PIXI_PACK_DEFAULT_VERSION").unwrap(), other_platform())]
        metadata_file: NamedTempFile,
    ) {
        let result = validate_metadata_file(metadata_file.path().to_path_buf()).await;
        let error = result.unwrap_err();
        assert_eq!(
            error.to_string(),
            "The pack was created for a different platform"
        );
    }
}
