use std::{
    env,
    fs::{create_dir_all, File},
    io::copy,
    path::{self, PathBuf},
    sync::Arc,
};

use futures::future::try_join_all;
use indicatif::ProgressStyle;
use rattler_conda_types::Platform;
use rattler_lock::{LockFile, Package};
use rattler_networking::{
    authentication_storage::{self, backends::file::FileStorageError},
    AuthenticationMiddleware, AuthenticationStorage,
};
use reqwest_middleware::ClientWithMiddleware;

use crate::PixiPackMetadata;

/* -------------------------------------------- PACK ------------------------------------------- */

/// Options for packing a pixi environment.
pub struct PackOptions {
    pub environment: String,
    pub platform: Platform,
    pub auth_file: Option<PathBuf>,
    pub output_dir: PathBuf,
    pub input_dir: PathBuf,
    pub metadata: PixiPackMetadata,
}

/// Pack a pixi environment.
pub async fn pack(options: PackOptions) -> Result<(), Box<dyn std::error::Error>> {
    let lockfile = LockFile::from_path(options.input_dir.join("pixi.lock").as_path()).unwrap();
    let client = reqwest_client_from_auth_storage(options.auth_file).unwrap();
    let env = lockfile.environment(&options.environment).unwrap();
    let packages = env.packages(options.platform).unwrap();

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
    let temp_dir = env::temp_dir();
    let download_dir = temp_dir.join("pixi-pack-tmp");
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
    let metadata_file = File::create(download_dir.join("pixi-pack.json"))?;
    serde_json::to_writer(metadata_file, &options.metadata)?;

    // Pack = archive + compress the contents.
    archive_directory(
        &download_dir,
        File::create(options.output_dir.join("environment.tar.zstd"))?,
    );

    // Clean up temporary download directory.
    std::fs::remove_dir_all(download_dir).expect("Could not remove temporary directory");

    // TODO: copy extra-files (parsed from pixi.toml), different compression algorithms, levels

    Ok(())
}

/* -------------------------------------- PACKAGE DOWNLOAD ------------------------------------- */

/// Get the authentication storage from the given auth file path.
fn get_auth_store(auth_file: Option<PathBuf>) -> Result<AuthenticationStorage, FileStorageError> {
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
fn reqwest_client_from_auth_storage(
    auth_file: Option<PathBuf>,
) -> Result<ClientWithMiddleware, FileStorageError> {
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
    client: ClientWithMiddleware,
    package: Package,
    output_dir: PathBuf,
    cb: Option<impl Fn() -> ()>,
) -> Result<(), Box<dyn std::error::Error>> {
    let conda_package = package.as_conda().unwrap();

    let output_dir = path::Path::new(&output_dir).join(&conda_package.package_record().subdir);
    create_dir_all(&output_dir)?;
    let file_name = output_dir.join(conda_package.url().path_segments().unwrap().last().unwrap());
    let mut dest = File::create(file_name)?;

    tracing::debug!("Fetching package {}", conda_package.url());
    let response = client.get(conda_package.url().to_string()).send().await?;
    let content = response.bytes().await?;

    copy(&mut content.as_ref(), &mut dest)?;
    if let Some(callback) = cb {
        callback();
    }

    Ok(())
}

/* ------------------------------------- COMPRESS + ARCHIVE ------------------------------------ */

/// Archive a directory into a compressed tarball.
fn archive_directory(input_dir: &PathBuf, archive_target: File) {
    // TODO: Allow different compression algorithms and levels.
    let compressor = zstd::stream::write::Encoder::new(archive_target, 0)
        .expect("could not create zstd encoder");

    let mut archive = tar::Builder::new(compressor);
    archive
        .append_dir_all("environment", input_dir)
        .expect("could not append directory to archive");

    let compressor = archive.into_inner().expect("could not write this archive");
    compressor.finish().expect("could not finish compression");
}
