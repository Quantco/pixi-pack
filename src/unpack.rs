use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::{Result, anyhow};
use futures::{
    TryFutureExt, TryStreamExt,
    stream::{self, StreamExt},
};
use indexmap::IndexMap;
use rattler::{
    install::{Installer, PythonInfo},
    package_cache::{CacheKey, CacheReporter, PackageCache},
};
use rattler_conda_types::{
    PackageRecord, Platform, RepoData, RepoDataRecord, package::DistArchiveIdentifier,
};
use rattler_networking::{LazyClient, retry_policies::default_retry_policy};
use rattler_package_streaming::fs::extract;
use rattler_shell::{
    activation::{ActivationVariables, Activator, PathModificationBehavior},
    shell::{Shell, ShellEnum},
};

use tar::Archive;
use tokio::fs;
use tokio_stream::wrappers::ReadDirStream;
use url::Url;
use uv_client::{BaseClientBuilder, Connectivity, RegistryClientBuilder};
use uv_configuration::{BuildOptions, NoBinary, NoBuild, initialize_rayon_once};
use uv_distribution::DistributionDatabase;
use uv_distribution_filename::{DistExtension, WheelFilename};
use uv_distribution_types::{Dist, Resolution};
use uv_installer::Preparer;
use uv_pep508::VerbatimUrl;
use uv_preview::Preview;
use uv_python::{Interpreter, PythonEnvironment};
use uv_types::{HashStrategy, InFlight};

use crate::{
    CHANNEL_DIRECTORY_NAME, DEFAULT_PIXI_PACK_VERSION, PIXI_PACK_METADATA_PATH, PIXI_PACK_VERSION,
    PYPI_DIRECTORY_NAME, PixiPackMetadata, ProgressReporter, REMOTE_PACKAGES_PATH,
    build_context::PixiPackBuildContext,
};

/// Options for unpacking a pixi environment.
#[derive(Debug, Clone)]
pub struct UnpackOptions {
    pub pack_file: PathBuf,
    pub output_directory: PathBuf,
    pub env_name: String,
    pub shell: Option<ShellEnum>,
    /// Updated while packages are downloaded and extracted, e.g. to show a progress bar.
    pub progress: Option<Arc<UnpackProgress>>,
}

/// Bytes of conda packages downloaded or extracted so far while unpacking.
#[derive(Debug, Default)]
pub struct UnpackProgress {
    done: AtomicU64,
    total: AtomicU64,
}

impl UnpackProgress {
    /// Bytes done and total bytes, the total is zero until it is known.
    pub fn get(&self) -> (u64, u64) {
        (
            self.done.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
        )
    }
}

/// Adds the bytes of one package download to [`UnpackProgress`].
struct DownloadReporter {
    progress: Arc<UnpackProgress>,
    reported: AtomicU64,
}

impl CacheReporter for DownloadReporter {
    fn on_validate_start(&self) -> usize {
        0
    }
    fn on_validate_complete(&self, _index: usize) {}
    fn on_download_start(&self) -> usize {
        // A retried download starts over.
        let reported = self.reported.swap(0, Ordering::Relaxed);
        self.progress.done.fetch_sub(reported, Ordering::Relaxed);
        0
    }
    fn on_download_progress(&self, _index: usize, progress: u64, _total: Option<u64>) {
        let reported = self.reported.swap(progress, Ordering::Relaxed);
        self.progress
            .done
            .fetch_add(progress.saturating_sub(reported), Ordering::Relaxed);
    }
    fn on_download_completed(&self, _index: usize) {}
}

/// Unpack a pixi environment.
pub async fn unpack(options: UnpackOptions) -> Result<()> {
    let tmp_dir = if !options.pack_file.is_dir() {
        Some(
            tempfile::tempdir()
                .map_err(|e| anyhow!("could not create temporary directory: {}", e))?,
        )
    } else {
        None
    };
    let unpack_dir = tmp_dir
        .as_ref()
        .map(|td| td.path())
        .unwrap_or(&options.pack_file);

    if let Some(td) = &tmp_dir {
        tracing::info!("Unarchiving pack to {}", td.path().display());
        unarchive(&options.pack_file, unpack_dir)
            .map_err(|e| anyhow!("Could not unarchive: {}", e))?;
    }

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
    initialize_rayon_once();

    let target_prefix = std::path::absolute(options.output_directory.join(options.env_name))
        .map_err(|e| anyhow!("Could not make path absolute: {e}"))?;
    tracing::info!("Creating prefix at {}", target_prefix.display());
    let channel_directory = unpack_dir.join(CHANNEL_DIRECTORY_NAME);
    let cache_dir = unpack_dir.join("cache");
    let remote_packages = read_remote_packages(&unpack_dir.join(REMOTE_PACKAGES_PATH)).await?;
    let progress = options.progress.unwrap_or_default();
    let packages = create_prefix(
        &channel_directory,
        &remote_packages,
        &target_prefix,
        &cache_dir,
        &progress,
    )
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

    if let Some(td) = tmp_dir {
        td.close()
            .map_err(|e| anyhow!("Could not remove temporary directory: {}", e))?;
    }

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

async fn collect_packages_in_subdir(
    subdir: PathBuf,
) -> Result<IndexMap<String, PackageRecord, ahash::RandomState>> {
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
    conda_packages.extend(repodata.packages);
    Ok(conda_packages
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect())
}

/// Read the URLs of packages that are downloaded instead of included in the pack, if any.
async fn read_remote_packages(path: &Path) -> Result<BTreeMap<String, Url>> {
    if !fs::try_exists(path).await? {
        return Ok(BTreeMap::new());
    }
    let contents = fs::read_to_string(path)
        .await
        .map_err(|e| anyhow!("could not read {}: {}", path.display(), e))?;
    serde_json::from_str(&contents)
        .map_err(|e| anyhow!("could not parse {}: {}", path.display(), e))
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
async fn collect_packages(
    channel_dir: &Path,
) -> Result<IndexMap<String, PackageRecord, ahash::RandomState>> {
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

/// Unarchive a tarball.
fn unarchive_generic<T: std::io::Read>(source: T, target_dir: &Path) -> Result<()> {
    let reader = std::io::BufReader::new(source);
    let mut archive = Archive::new(reader);

    archive
        .unpack(target_dir)
        .map_err(|e| anyhow!("could not unpack archive: {}", e))?;

    Ok(())
}

pub fn unarchive(archive_path: &Path, target_dir: &Path) -> Result<()> {
    if archive_path == Path::new("-") {
        unarchive_generic(std::io::stdin(), target_dir)
    } else {
        let file = std::fs::File::open(archive_path)
            .map_err(|e| anyhow!("could not open archive {:#?}: {}", archive_path, e))?;
        unarchive_generic(file, target_dir)
    }
}

async fn create_prefix(
    channel_dir: &Path,
    remote_packages: &BTreeMap<String, Url>,
    target_prefix: &Path,
    cache_dir: &Path,
    progress: &Arc<UnpackProgress>,
) -> Result<IndexMap<String, PackageRecord, ahash::RandomState>> {
    let packages = collect_packages(channel_dir)
        .await
        .map_err(|e| anyhow!("could not collect packages: {}", e))?;
    let total = packages.values().filter_map(|p| p.size).sum();
    progress.total.store(total, Ordering::Relaxed);

    if remote_packages.is_empty() {
        eprintln!(
            "⏳ Extracting and installing {} packages to {}...",
            packages.len(),
            cache_dir.display()
        );
    } else {
        eprintln!(
            "⏳ Downloading {} and extracting {} packages to {}...",
            remote_packages.len(),
            packages.len() - remote_packages.len(),
            cache_dir.display()
        );
    }
    let reporter = ProgressReporter::new(packages.len() as u64);

    // extract packages to cache
    tracing::info!("Creating cache with {} packages", packages.len());
    let package_cache = PackageCache::new(cache_dir);
    let client = LazyClient::from(reqwest::Client::new());

    let repodata_records: Vec<RepoDataRecord> = stream::iter(packages.clone())
        .map(|(file_name, package_record)| {
            let cache_key = CacheKey::from(&package_record);
            let remote_url = remote_packages
                .get(&format!("{}/{}", package_record.subdir, file_name))
                .cloned();
            let package_path = channel_dir.join(&package_record.subdir).join(&file_name);
            let url = match &remote_url {
                Some(url) => url.clone(),
                None => {
                    let normalized_path = package_path.canonicalize().unwrap();
                    Url::from_file_path(&normalized_path)
                        .map_err(|_| {
                            anyhow!(
                                "could not convert path to URL: {}",
                                normalized_path.display()
                            )
                        })
                        .unwrap()
                }
            };

            tracing::debug!(
                "Extracting package {} with URL {}",
                package_record.name.as_normalized(),
                url
            );

            let repodata_record = RepoDataRecord {
                package_record,
                identifier: file_name
                    .parse::<DistArchiveIdentifier>()
                    .expect("package filename should be a valid archive identifier"),
                url,
                channel: None,
            };

            let size = repodata_record.package_record.size.unwrap_or(0);
            let client = client.clone();
            let package_cache = &package_cache;
            let reporter = &reporter;
            async move {
                // We have to prepare the package cache by inserting all packages into it.
                // We can only do so by calling `get_or_fetch` on each package, which will
                // use the provided closure to fetch the package and insert it into the cache.
                // Downloaded packages are checked against the SHA256 of their record.
                let is_remote = remote_url.is_some();
                let result = match remote_url {
                    Some(url) => {
                        package_cache
                            .get_or_fetch_from_url_with_retry(
                                cache_key,
                                url,
                                client,
                                default_retry_policy(),
                                Some(Arc::new(DownloadReporter {
                                    progress: progress.clone(),
                                    reported: AtomicU64::new(0),
                                })),
                                None,
                            )
                            .await
                    }
                    None => {
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
                    }
                };
                result.map_err(|e| {
                    anyhow!(
                        "could not extract \"{}\": {}",
                        repodata_record.as_ref().name.as_source(),
                        e
                    )
                })?;
                reporter.pb.inc(1);
                if !is_remote {
                    progress.done.fetch_add(size, Ordering::Relaxed);
                }

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
    installed_conda_packages: IndexMap<String, PackageRecord, ahash::RandomState>,
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

    let client = RegistryClientBuilder::new(
        BaseClientBuilder::default().connectivity(Connectivity::Offline),
        pypi_cache.clone(),
    )
    .build()
    .map_err(|e| anyhow!("Could not build registry client: {}", e))?;
    let context = PixiPackBuildContext::new(pypi_cache.clone());
    let downloads_semaphore = Arc::new(tokio::sync::Semaphore::new(1));
    let distribute_database = DistributionDatabase::new(&client, &context, downloads_semaphore);
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
    uv_installer::Installer::new(&venv, Preview::default())
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
