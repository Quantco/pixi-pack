use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use tempfile::NamedTempFile;
use tokio::{fs::create_dir_all, fs::File, io::AsyncWriteExt};

use anyhow::Result;
use async_compression::{tokio::write::ZstdEncoder, Level};
use futures::{
    stream::{self},
    StreamExt, TryStreamExt,
};
use indicatif::ProgressStyle;
use rattler_conda_types::Platform;
use rattler_lock::{LockFile, Package};
use rattler_networking::{AuthenticationMiddleware, AuthenticationStorage};
use reqwest_middleware::ClientWithMiddleware;
use tokio_tar::Builder;

use crate::{PixiPackMetadata, PIXI_PACK_METADATA_PATH, PKGS_DIR};
use anyhow::anyhow;

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
    pub level: Option<Level>,
}

/// Pack a pixi environment.
pub async fn pack(options: PackOptions) -> Result<()> {
    let lockfile = LockFile::from_path(
        options
            .manifest_path
            .parent()
            .ok_or(anyhow!("could not get parent directory"))?
            .join("pixi.lock")
            .as_path(),
    )
    .map_err(|e| anyhow!("could not read lockfile: {e}"))?;

    let client = reqwest_client_from_auth_storage(options.auth_file)
        .map_err(|e| anyhow!("could not create reqwest client from auth storage: {e}"))?;

    let env = lockfile.environment(&options.environment).ok_or(anyhow!(
        "environment not found in lockfile: {}",
        options.environment
    ))?;

    let packages = env.packages(options.platform).ok_or(anyhow!(
        "platform not found in lockfile: {}",
        options.platform.as_str()
    ))?;

    // Download packages to temporary directory.
    tracing::info!("Downloading {} packages", packages.len());
    let bar = indicatif::ProgressBar::new(packages.len() as u64);
    bar.set_style(
        ProgressStyle::with_template(
            "[{elapsed_precise}] {bar:40.cyan/blue} {pos:>7}/{len:7} {msg}",
        )
        .expect("could not set progress style")
        .progress_chars("##-"),
    );

    let output_pack_path =
        tempfile::tempdir().map_err(|e| anyhow!("could not create temporary directory: {}", e))?;
    let pkg_download_dir = output_pack_path.path();

    create_dir_all(&pkg_download_dir).await?;

    stream::iter(packages)
        .map(Ok)
        .try_for_each_concurrent(50, |package| async {
            download_package(&client, package, pkg_download_dir).await?;

            bar.inc(1);

            Ok(())
        })
        .await
        .map_err(|e: anyhow::Error| anyhow!("could not download package: {}", e))?;

    bar.finish();

    // Add pixi-pack.json containing metadata.
    let mut metadata_file = NamedTempFile::new()?;
    serde_json::to_writer(&mut metadata_file, &options.metadata)?;

    // Pack = archive + compress the contents.
    archive_directory(
        output_pack_path.path(),
        File::create(options.output_file).await?,
        metadata_file.path(),
        options.level,
    )
    .await
    .map_err(|e| anyhow!("could not archive directory: {}", e))?;

    // TODO: copy extra-files (parsed from pixi.toml), different compression algorithms, levels
    // todo: fail on pypi deps

    Ok(())
}

/* -------------------------------------- PACKAGE DOWNLOAD ------------------------------------- */

/// Get the authentication storage from the given auth file path.
fn get_auth_store(auth_file: Option<PathBuf>) -> Result<AuthenticationStorage> {
    match auth_file {
        Some(auth_file) => Ok(AuthenticationStorage::from_file(&auth_file)?),
        None => Ok(rattler_networking::AuthenticationStorage::default()),
    }
}

/// Create a reqwest client (optionally including authentication middleware).
fn reqwest_client_from_auth_storage(auth_file: Option<PathBuf>) -> Result<ClientWithMiddleware> {
    let auth_storage = get_auth_store(auth_file)?;

    let timeout = 5 * 60;
    Ok(reqwest_middleware::ClientBuilder::new(
        reqwest::Client::builder()
            .no_gzip()
            .pool_max_idle_per_host(20)
            .user_agent("pixi-pack")
            .timeout(std::time::Duration::from_secs(timeout))
            .build()
            .expect("failed to create client"),
    )
    .with_arc(Arc::new(AuthenticationMiddleware::new(auth_storage)))
    .build())
}

/// Download a conda package to a given output directory.
async fn download_package(
    client: &ClientWithMiddleware,
    package: Package,
    output_dir: &Path,
) -> Result<()> {
    let conda_package = package
        .as_conda()
        .ok_or(anyhow!("package is not a conda package"))?;

    let file_name = conda_package
        .file_name()
        .ok_or(anyhow!("could not get file name"))?;
    let mut dest = File::create(output_dir.join(file_name)).await?;

    tracing::debug!("Fetching package {}", conda_package.url());
    let mut response = client.get(conda_package.url().to_string()).send().await?;

    while let Some(chunk) = response.chunk().await? {
        dest.write_all(&chunk).await?;
    }

    Ok(())
}

/* ------------------------------------- COMPRESS + ARCHIVE ------------------------------------ */

/// Archive a directory into a compressed tarball.
async fn archive_directory(
    package_dir: &Path,
    archive_target: File,
    pixi_pack_metadata_path: &Path,
    level: Option<Level>,
) -> Result<()> {
    let writer = tokio::io::BufWriter::new(archive_target);

    let level = level.unwrap_or(Level::Default);
    let compressor = ZstdEncoder::with_quality(writer, level);

    let mut archive = Builder::new(compressor);

    archive
        .append_path_with_name(pixi_pack_metadata_path, PIXI_PACK_METADATA_PATH)
        .await
        .map_err(|e| anyhow!("could not append metadata file to archive: {}", e))?;

    archive
        .append_dir_all(PKGS_DIR, package_dir)
        .await
        .map_err(|e| anyhow!("could not append directory to archive: {}", e))?;

    let mut compressor = archive
        .into_inner()
        .await
        .map_err(|e| anyhow!("could not finish writing archive: {}", e))?;

    compressor
        .shutdown()
        .await
        .map_err(|e| anyhow!("could not flush output: {}", e))?;

    Ok(())
}
