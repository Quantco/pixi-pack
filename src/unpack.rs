use std::{
    io::Cursor,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Result};
use futures::{
    stream::{self, StreamExt},
    TryFutureExt, TryStreamExt,
};
use fxhash::FxHashMap;
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

use tokio::fs;
use tokio::io::AsyncReadExt;
use tokio_stream::wrappers::ReadDirStream;
use tokio_tar::Archive;
use url::Url;

use crate::{
    PixiPackMetadata, ProgressReporter, CHANNEL_DIRECTORY_NAME, DEFAULT_PIXI_PACK_VERSION,
    PIXI_PACK_METADATA_PATH,
};

/// Options for unpacking a pixi environment.
#[derive(Debug, Clone)]
pub struct UnpackOptions {
    pub pack_file: PathBuf,
    pub output_directory: PathBuf,
    pub shell: Option<ShellEnum>,
}

/// Unpack a pixi environment.
pub async fn unpack(options: UnpackOptions) -> Result<()> {
    let tmp_dir =
        tempfile::tempdir().map_err(|e| anyhow!("Could not create temporary directory: {}", e))?;
    let unpack_dir = tmp_dir.path();

    tracing::info!("Unarchiving pack to {}", unpack_dir.display());
    if options.pack_file.extension().unwrap_or_default() == "sh" {
        unarchive_from_shellscript(&options.pack_file, unpack_dir)
            .await
            .map_err(|e| anyhow!("Could not extract archive from shell script: {}", e))?;
    } else {
        unarchive(&options.pack_file, unpack_dir)
            .await
            .map_err(|e| anyhow!("Could not unarchive: {}", e))?;
    }

    validate_metadata_file(unpack_dir.join(PIXI_PACK_METADATA_PATH)).await?;

    let target_prefix = options.output_directory.join("env");

    tracing::info!("Creating prefix at {}", target_prefix.display());
    let channel_directory = unpack_dir.join(CHANNEL_DIRECTORY_NAME);
    let cache_dir = unpack_dir.join("cache");
    create_prefix(&channel_directory, &target_prefix, &cache_dir)
        .await
        .map_err(|e| anyhow!("Could not create prefix: {}", e))?;

    tracing::info!("Generating activation script");
    create_activation_script(
        &options.output_directory,
        &target_prefix,
        options.shell.unwrap_or_default(),
    )
    .await
    .map_err(|e| anyhow!("Could not create activation script: {}", e))?;

    tracing::info!(
        "Finished unpacking to {}.",
        options.output_directory.display(),
    );
    eprintln!(
        "💫 Finished unpacking to {}.",
        options.output_directory.display()
    );

    tmp_dir
        .close()
        .map_err(|e| anyhow!("Could not remove temporary directory: {}", e))?;

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

    if metadata.version != DEFAULT_PIXI_PACK_VERSION {
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

// Extract the archive from a shell script and unar
pub async fn unarchive_from_shellscript(shell_script_path: &Path, target_dir: &Path) -> Result<()> {
    let mut shell_script = fs::File::open(shell_script_path)
        .await
        .map_err(|e| anyhow!("could not open shell script: {}", e))?;

    let mut content = Vec::new();
    shell_script
        .read_to_end(&mut content)
        .await
        .map_err(|e| anyhow!("could not read shell script: {}", e))?;

    let end_header = b"@@END_HEADER@@\n";
    let end_archive = b"@@END_ARCHIVE@@\n";

    let start = content
        .windows(end_header.len())
        .position(|window| window == end_header)
        .map(|pos| pos + end_header.len())
        .ok_or_else(|| anyhow!("Could not find @@END_HEADER@@ in shell script"))?;

    let end = content
        .windows(end_archive.len())
        .position(|window| window == end_archive)
        .ok_or_else(|| anyhow!("Could not find @@END_ARCHIVE@@ in shell script"))?;

    let archive_content = &content[start..end];

    let reader = tokio::io::BufReader::new(Cursor::new(archive_content));
    let mut archive = Archive::new(reader);

    archive
        .unpack(target_dir)
        .await
        .map_err(|e| anyhow!("could not unpack archive: {}", e))?;

    Ok(())
}

/// Unarchive a tarball.
pub async fn unarchive(archive_path: &Path, target_dir: &Path) -> Result<()> {
    let file = fs::File::open(archive_path)
        .await
        .map_err(|e| anyhow!("could not open archive {:#?}: {}", archive_path, e))?;

    let reader = tokio::io::BufReader::new(file);
    let mut archive = Archive::new(reader);

    archive
        .unpack(target_dir)
        .await
        .map_err(|e| anyhow!("could not unpack archive: {}", e))?;

    Ok(())
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
    let reporter = ProgressReporter::new(packages.len() as u64);

    // extract packages to cache
    tracing::info!("Creating cache with {} packages", packages.len());
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
                            |destination| async move {
                                extract(&package_path, &destination).map(|_| ())
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
                reporter.pb.inc(1);

                Ok::<RepoDataRecord, anyhow::Error>(repodata_record)
            }
        })
        .buffer_unordered(50)
        .try_collect()
        .await?;

    // Invariant: all packages are in the cache
    tracing::info!("Installing {} packages", repodata_records.len());
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

async fn create_activation_script(
    destination: &Path,
    prefix: &Path,
    shell: ShellEnum,
) -> Result<()> {
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
        #[default(DEFAULT_PIXI_PACK_VERSION.to_string())] version: String,
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
        #[with(DEFAULT_PIXI_PACK_VERSION.to_string(), other_platform())]
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
