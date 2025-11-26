use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, LazyLock},
};

use anyhow::{Result, anyhow};
use either::Either;
use futures::{
    TryFutureExt, TryStreamExt,
    stream::{self, StreamExt},
};
use fxhash::FxHashMap;
use rattler::{
    install::{Installer, PythonInfo},
    package_cache::{CacheKey, PackageCache},
};
use rattler_conda_types::{PackageRecord, Platform, RepoData, RepoDataRecord};
use rattler_package_streaming::fs::extract;
use rattler_shell::{
    activation::{ActivationVariables, Activator, PathModificationBehavior},
    shell::{Shell, ShellEnum},
};

use tar::Archive;
use tokio::fs;
use tokio_stream::wrappers::ReadDirStream;
use url::Url;
use uv_client::{BaseClientBuilder, RegistryClientBuilder};
use uv_configuration::{BuildOptions, NoBinary, NoBuild, RAYON_INITIALIZE};
use uv_distribution::DistributionDatabase;
use uv_distribution_filename::{DistExtension, WheelFilename};
use uv_distribution_types::{Dist, Resolution};
use uv_installer::Preparer;
use uv_pep508::VerbatimUrl;
use uv_preview::{Preview, PreviewFeatures};
use uv_python::{Interpreter, PythonEnvironment};
use uv_types::{HashStrategy, InFlight};

use crate::{
    CHANNEL_DIRECTORY_NAME, DEFAULT_PIXI_PACK_VERSION, PIXI_PACK_METADATA_PATH, PIXI_PACK_VERSION,
    PYPI_DIRECTORY_NAME, PixiPackMetadata, ProgressReporter, build_context::PixiPackBuildContext,
};

/// Options for unpacking a pixi environment.
#[derive(Debug, Clone)]
pub struct UnpackOptions {
    pub pack_file: PathBuf,
    pub output_directory: PathBuf,
    pub env_name: String,
    pub shell: Option<ShellEnum>,
}

/// Unpack a pixi environment.
pub async fn unpack(options: UnpackOptions) -> Result<()> {
    let tmp_dir =
        tempfile::tempdir().map_err(|e| anyhow!("Could not create temporary directory: {}", e))?;
    let unpack_dir = tmp_dir.path();

    tracing::info!("Unarchiving pack to {}", unpack_dir.display());

    unarchive(&options.pack_file, unpack_dir)
        .await
        .map_err(|e| anyhow!("Could not unarchive: {}", e))?;

    validate_metadata_file(unpack_dir.join(PIXI_PACK_METADATA_PATH)).await?;

    // HACK: The `Installer` and `Preparer` created below (in `install_pypi_packages`),
    // will utilize rayon for parallelism. By using rayon
    // it will implicitly initialize a global thread pool.
    // However, uv has a mechanism to initialize
    // rayon itself, which will crash if the global thread pool was
    // already initialized. To prevent this, we force uv the initialize
    // the rayon global thread pool, this ensures that any rayon code
    // that is run will use the same thread pool.
    //
    // One downside of this approach is that perhaps it turns out that we won't need
    // the thread pool at all (because no changes needed to happen for instance).
    // There is a little bit of overhead when that happens, but I don't see another
    // way around that.
    // xref https://github.com/rayon-rs/rayon/issues/93
    // xref https://github.com/prefix-dev/pixi/blob/4dc02c840d63e75f16a2da6a8fc74a7f67218cb3/src/environment/conda_prefix.rs#L294
    LazyLock::force(&RAYON_INITIALIZE);

    let target_prefix = std::path::absolute(options.output_directory.join(options.env_name))
        .map_err(|e| anyhow!("Could not make path absolute: {e}"))?;
    tracing::info!("Creating prefix at {}", target_prefix.display());
    let channel_directory = unpack_dir.join(CHANNEL_DIRECTORY_NAME);
    let cache_dir = unpack_dir.join("cache");
    let packages = create_prefix(&channel_directory, &target_prefix, &cache_dir)
        .await
        .map_err(|e| anyhow!("Could not create prefix: {}", e))?;

    install_pypi_packages(unpack_dir, &target_prefix, packages)
        .await
        .map_err(|e| anyhow!("Could not install all pypi packages: {}", e))?;

    tracing::info!("Generating activation script");
    create_activation_script(
        &options.output_directory,
        &target_prefix,
        options.shell.unwrap_or_default(),
    )
    .await
    .map_err(|e| anyhow!("Could not create activation script: {}", e))?;

    tmp_dir
        .close()
        .map_err(|e| anyhow!("Could not remove temporary directory: {}", e))?;

    tracing::info!(
        "Finished unpacking to {}.",
        options.output_directory.display(),
    );
    eprintln!(
        "💫 Finished unpacking to {}.",
        options.output_directory.display()
    );

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

    tracing::debug!("pack metadata: {:?}", metadata);
    if metadata.pixi_pack_version != Some(PIXI_PACK_VERSION.to_string()) {
        tracing::warn!(
            "The pack was created with a different version of pixi-pack: {:?}",
            metadata.pixi_pack_version
        );
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

fn open_input_file(target: &Path) -> Result<Either<std::io::Stdin,std::fs::File>> {
    if target == "-" {
        // Use stdin
        Ok(either::Left(std::io::stdin()))
    } else {
        Ok(either::Right(std::fs::File::open(&target)?))
    }
}

/// Unarchive a tarball.
pub async fn unarchive(archive_path: &Path, target_dir: &Path) -> Result<()> {
    let file = open_input_file(archive_path)
        .map_err(|e| anyhow!("could not open archive {:#?}: {}", archive_path, e))?;

    let reader = std::io::BufReader::new(file);
    let mut archive = Archive::new(reader);

    archive
        .unpack(target_dir)
        .map_err(|e| anyhow!("could not unpack archive: {}", e))?;

    Ok(())
}

async fn create_prefix(
    channel_dir: &Path,
    target_prefix: &Path,
    cache_dir: &Path,
) -> Result<FxHashMap<String, PackageRecord>> {
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

    let repodata_records: Vec<RepoDataRecord> = stream::iter(packages.clone())
        .map(|(file_name, package_record)| {
            let cache_key = CacheKey::from(&package_record);

            let package_path = channel_dir.join(&package_record.subdir).join(&file_name);
            let normalized_path = package_path.canonicalize().unwrap();

            let url = Url::from_file_path(&normalized_path)
                .map_err(|_| {
                    anyhow!(
                        "could not convert path to URL: {}",
                        normalized_path.display()
                    )
                })
                .unwrap();

            tracing::debug!(
                "Extracting package {} with URL {}",
                package_record.name.as_normalized(),
                url
            );

            let repodata_record = RepoDataRecord {
                package_record,
                file_name,
                url,
                channel: None,
            };

            async {
                // We have to prepare the package cache by inserting all packages into it.
                // We can only do so by calling `get_or_fetch` on each package, which will
                // use the provided closure to fetch the package and insert it into the cache.
                package_cache
                    .get_or_fetch(
                        cache_key,
                        move |destination| {
                            let value = package_path.clone();
                            async move { extract(&value, &destination).map(|_| ()) }
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
        .buffer_unordered(rattler_config::config::concurrency::default_max_concurrent_downloads())
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

    Ok(packages)
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
        current_env: HashMap::new(),
    })?;

    let contents = result.script.contents()?;
    fs::write(activate_path, contents)
        .await
        .map_err(|e| anyhow!("Could not write activate script: {}", e))?;

    Ok(())
}

async fn install_pypi_packages(
    unpack_dir: &Path,
    target_prefix: &Path,
    installed_conda_packages: FxHashMap<String, PackageRecord>,
) -> Result<()> {
    let pypi_directory = unpack_dir.join(PYPI_DIRECTORY_NAME);
    if !pypi_directory.exists() {
        return Ok(());
    }
    tracing::info!("Installing pypi packages");

    // Find installed python in this prefix
    let python_record = installed_conda_packages
        .values()
        .find(|x| x.name.as_normalized() == "python");
    let python_record = python_record.ok_or_else(|| anyhow!("No python record found."))?;
    let python_info = PythonInfo::from_python_record(python_record, Platform::current())?;
    tracing::debug!("Current Python is: {:?}", python_info);
    let pypi_cache =
        uv_cache::Cache::temp().map_err(|e| anyhow!("Could not create cache folder: {}", e))?;
    // Find a working python interpreter
    let interpreter = Interpreter::query(target_prefix.join(python_info.path()), &pypi_cache)
        .map_err(|e| anyhow!("Could not load python interpreter: {}", e))?;
    let tags = interpreter.tags()?.clone();
    let venv = PythonEnvironment::from_interpreter(interpreter);
    // Collect all whl files in directory
    let wheels = collect_pypi_packages(&pypi_directory)
        .await
        .map_err(|e| anyhow!("Could not find all pypi package files: {}", e))?;
    eprintln!(
        "⏳ Extracting and installing {} pypi packages to {}...",
        wheels.len(),
        venv.root().display(),
    );

    let client =
        RegistryClientBuilder::new(BaseClientBuilder::default(), pypi_cache.clone()).build();
    let context = PixiPackBuildContext::new(pypi_cache.clone());
    let distribute_database = DistributionDatabase::new(&client, &context, 1usize);
    let build_options = BuildOptions::new(NoBinary::None, NoBuild::All);
    let preparer = Preparer::new(
        &pypi_cache,
        &tags,
        &HashStrategy::None,
        &build_options,
        distribute_database,
    );
    let resolution = Resolution::default();
    let inflight = InFlight::default();
    // unzip all wheel packages
    let unzipped_dists = preparer
        .prepare(wheels.clone(), &inflight, &resolution)
        .await
        .map_err(|e| anyhow!("Could not unzip all pypi packages: {}", e))?;
    // install all wheel packages
    uv_installer::Installer::new(&venv, Preview::new(PreviewFeatures::default()))
        .install(unzipped_dists)
        .await
        .map_err(|e| anyhow!("Could not install all pypi packages: {}", e))?;

    Ok(())
}

async fn collect_pypi_packages(package_dir: &Path) -> Result<Vec<Arc<Dist>>> {
    let mut entries = fs::read_dir(package_dir)
        .await
        .map_err(|e| anyhow!("could not read pypi directory: {}", e))?;
    let mut ret = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        tracing::trace!("Processing file: {:?}", entry.path());
        let file_name = entry
            .file_name()
            .into_string()
            .map_err(|x| anyhow!("cannot convert filename into string {:?}", x))?;
        let wheel_file_name = WheelFilename::from_str(&file_name)?;
        let dist = Arc::new(Dist::from_file_url(
            wheel_file_name.name.clone(),
            VerbatimUrl::from_absolute_path(entry.path().clone())?,
            entry.path().as_path(),
            DistExtension::Wheel,
        )?);
        ret.push(dist);
    }

    Ok(ret)
}

/* --------------------------------------------------------------------------------------------- */
/*                                             TESTS                                             */
/* --------------------------------------------------------------------------------------------- */

#[cfg(test)]
mod tests {
    use crate::PIXI_PACK_VERSION;

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
        let metadata = PixiPackMetadata {
            version,
            pixi_pack_version: Some(PIXI_PACK_VERSION.to_string()),
            platform,
        };
        let buffer = metadata_file.as_file_mut();
        buffer
            .write_all(json!(metadata).to_string().as_bytes())
            .unwrap();
        metadata_file
    }

    #[rstest]
    #[tokio::test]
    async fn test_metadata_file_valid(metadata_file: NamedTempFile) {
        assert!(
            validate_metadata_file(metadata_file.path().to_path_buf())
                .await
                .is_ok()
        )
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
