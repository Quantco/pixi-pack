use core::fmt;
use std::{
    fs::{create_dir_all, File},
    io::copy,
    path::{self, PathBuf},
    sync::Arc,
};

use derive_more::From;

use futures::future::try_join_all;
use indicatif::ProgressStyle;
use rattler_conda_types::Platform;
use rattler_lock::{LockFile, Package};
use rattler_networking::{
    authentication_storage::{self, backends::file::FileStorageError},
    AuthenticationMiddleware, AuthenticationStorage,
};
use reqwest_middleware::ClientWithMiddleware;
use tempdir::TempDir;
use url::Url;

use crate::{PixiPackMetadata, CHANNEL_DIRECTORY_NAME};

pub type Result<T> = std::result::Result<T, PackError>;

#[derive(Debug, From)]
pub enum PackError {
    #[from]
    ParseCondaLockError(rattler_lock::ParseCondaLockError),
    EnvironmentNotAvailable(String),
    PlatformNotAvailable(Platform),
    IncorrectCondaPackageUrl(Url),
    IncorrectManifestPath,
    #[from]
    AuthStoreError(FileStorageError),
    #[from]
    Io(std::io::Error),
    PixiMetadataSerialization(serde_json::Error),
    CreateClient(reqwest::Error),
    SendRequestError(reqwest_middleware::Error),
    CollectRequestError(reqwest::Error),
}
impl fmt::Display for PackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackError::ParseCondaLockError(e) => write!(
                f,
                "An error occurred while parsing the pixi.lock file: {}",
                e
            ),
            PackError::IncorrectCondaPackageUrl(url) => {
                write!(f, "Incorrect conda package URL: {}", url)
            }
            PackError::EnvironmentNotAvailable(env) => {
                write!(f, "The environment {} is not available", env)
            }
            PackError::PlatformNotAvailable(platform) => {
                write!(f, "The platform {} is not available", platform)
            }
            PackError::IncorrectManifestPath => write!(f, "The manifest path is incorrect"),
            PackError::AuthStoreError(e) => write!(
                f,
                "An error occurred while getting the authentication storage: {}",
                e
            ),
            PackError::Io(e) => write!(f, "An I/O error occurred: {}", e),
            PackError::PixiMetadataSerialization(e) => write!(
                f,
                "An error occurred while serializing pixi-pack.json: {}",
                e
            ),
            PackError::CreateClient(e) => {
                write!(f, "An error occurred while creating the client: {}", e)
            }
            PackError::SendRequestError(e) => {
                write!(f, "An error occurred while sending the request: {}", e)
            }
            PackError::CollectRequestError(e) => write!(
                f,
                "An error occurred while collecting the data for the request: {}",
                e
            ),
        }
    }
}
impl std::error::Error for PackError {}

/* -------------------------------------------- PACK ------------------------------------------- */

/// Options for packing a pixi environment.
#[derive(Debug)]
pub struct PackOptions {
    pub environment: String,
    pub platform: Platform,
    pub auth_file: Option<PathBuf>,
    pub output_file: PathBuf,
    pub manifest_path: PathBuf,
    pub metadata: PixiPackMetadata,
}

/// Pack a pixi environment.
pub async fn pack(options: PackOptions) -> Result<()> {
    let lockfile = LockFile::from_path(
        options
            .manifest_path
            .parent()
            .ok_or(PackError::IncorrectManifestPath)?
            .join("pixi.lock")
            .as_path(),
    )?;
    let client = reqwest_client_from_auth_storage(options.auth_file)?;
    let env = lockfile
        .environment(&options.environment)
        .ok_or(PackError::EnvironmentNotAvailable(options.environment))?;
    let packages = env
        .packages(options.platform)
        .ok_or(PackError::PlatformNotAvailable(options.platform))?;

    // Download packages to temporary directory.
    tracing::info!("Downloading {} packages", packages.len());
    let bar = indicatif::ProgressBar::new(packages.len() as u64);
    bar.set_style(
        ProgressStyle::with_template(
            "[{elapsed_precise}] {bar:40.cyan/blue} {pos:>7}/{len:7} {msg}",
        )
        .unwrap()
        .progress_chars("##-"),
    );
    let download_dir = TempDir::new("pixi-pack-download")?.into_path();
    try_join_all(packages.into_iter().map(|package| {
        download_package(
            client.clone(),
            package,
            download_dir.clone(),
            Some(|| bar.inc(1)),
        )
    }))
    .await?;
    bar.finish();

    // Create `repodata.json` files.
    rattler_index::index(download_dir.as_path(), None)?;

    // Add pixi-pack.json containing metadata.
    let metadata_path = download_dir.join("pixi-pack.json");
    let metadata_file = File::create(metadata_path.clone())?;
    serde_json::to_writer(metadata_file, &options.metadata)
        .map_err(PackError::PixiMetadataSerialization)?;

    // Pack = archive + compress the contents.
    archive_directory(
        &download_dir,
        File::create(options.output_file)?,
        &metadata_path,
    )?;

    // Clean up temporary download directory.
    std::fs::remove_dir_all(download_dir)?;

    // TODO: different compression algorithms, levels
    Ok(())
}

/* -------------------------------------- PACKAGE DOWNLOAD ------------------------------------- */

/// Get the authentication storage from the given auth file path.
fn get_auth_store(auth_file: Option<PathBuf>) -> Result<AuthenticationStorage> {
    match auth_file {
        Some(auth_file) => {
            let mut store = AuthenticationStorage::new();
            store.add_backend(Arc::from(
                authentication_storage::backends::file::FileStorage::new(auth_file)?,
            ));
            Ok(store)
        }
        None => Ok(rattler_networking::AuthenticationStorage::default()),
    }
}

/// Create a reqwest client (optionally including authentication middleware).
fn reqwest_client_from_auth_storage(auth_file: Option<PathBuf>) -> Result<ClientWithMiddleware> {
    let auth_storage = get_auth_store(auth_file)?;

    let timeout = 5 * 60;
    let client = reqwest_middleware::ClientBuilder::new(
        reqwest::Client::builder()
            .no_gzip()
            .pool_max_idle_per_host(20)
            .user_agent("pixi-pack")
            .timeout(std::time::Duration::from_secs(timeout))
            .build()
            .map_err(PackError::CreateClient)?,
    )
    .with_arc(Arc::new(AuthenticationMiddleware::new(auth_storage)))
    .build();
    Ok(client)
}

/// Download a conda package to a given output directory.
async fn download_package(
    client: ClientWithMiddleware,
    package: Package,
    output_dir: PathBuf,
    cb: Option<impl Fn()>,
) -> Result<()> {
    let conda_package = match package {
        Package::Conda(package) => package,
        Package::Pypi(package) => {
            let package_name = &package.data().package.name;
            tracing::warn!("Skipping pypi package: {:?}", package_name);
            return Ok(());
        }
    };

    let output_dir = path::Path::new(&output_dir).join(&conda_package.package_record().subdir);
    create_dir_all(&output_dir)?;
    let conda_package_url = conda_package.url();
    let file_name = output_dir.join(
        conda_package_url
            .path_segments()
            .ok_or(PackError::IncorrectCondaPackageUrl(
                conda_package_url.clone(),
            ))?
            .last()
            .ok_or(PackError::IncorrectCondaPackageUrl(
                conda_package_url.clone(),
            ))?,
    );
    let mut dest = File::create(file_name)?;

    tracing::debug!("Fetching package {}", conda_package.url());
    let response = client
        .get(conda_package.url().to_string())
        .send()
        .await
        .map_err(PackError::SendRequestError)?;
    let content = response
        .bytes()
        .await
        .map_err(PackError::CollectRequestError)?;

    copy(&mut content.as_ref(), &mut dest)?;
    if let Some(callback) = cb {
        callback();
    }

    Ok(())
}

/* ------------------------------------- COMPRESS + ARCHIVE ------------------------------------ */

/// Archive a directory into a compressed tarball.
fn archive_directory(
    input_dir: &PathBuf,
    archive_target: File,
    pixi_pack_metadata_path: &PathBuf,
) -> std::io::Result<File> {
    // TODO: Allow different compression algorithms and levels.
    let compressor = zstd::stream::write::Encoder::new(archive_target, 0)?;

    let mut archive = tar::Builder::new(compressor);
    archive.append_path_with_name(pixi_pack_metadata_path, "pixi-pack.json")?;
    archive.append_dir_all(CHANNEL_DIRECTORY_NAME, input_dir)?;

    let compressor = archive.into_inner()?;
    compressor.finish()
}
