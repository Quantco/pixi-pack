use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use tokio::{
    fs::{self, create_dir_all, File},
    io::AsyncWriteExt,
};

use anyhow::Result;
use async_compression::{tokio::write::ZstdEncoder, Level};
use futures::{stream, StreamExt, TryFutureExt, TryStreamExt};
use indicatif::ProgressStyle;
use rattler_conda_types::{ChannelInfo, PackageRecord, Platform, RepoData};
use rattler_lock::{CondaPackage, LockFile, Package};
use rattler_networking::{AuthenticationMiddleware, AuthenticationStorage};
use reqwest_middleware::ClientWithMiddleware;
use tokio_tar::Builder;

use crate::{PixiPackMetadata, CHANNEL_DIRECTORY_NAME, PIXI_PACK_METADATA_PATH};
use anyhow::anyhow;

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
    let lockfile_path = options
        .manifest_path
        .parent()
        .ok_or(anyhow!("could not get parent directory"))?
        .join("pixi.lock");

    let lockfile = LockFile::from_path(&lockfile_path).map_err(|e| {
        anyhow!(
            "could not read lockfile at {}: {}",
            lockfile_path.display(),
            e
        )
    })?;

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

    let mut conda_packages = Vec::new();

    for package in packages {
        match package {
            Package::Conda(p) => conda_packages.push(p),
            Package::Pypi(_) => {
                anyhow::bail!("pypi packages are not supported in pixi-pack");
            }
        }
    }

    // Download packages to temporary directory.
    tracing::info!("Downloading {} packages", conda_packages.len());
    let bar = indicatif::ProgressBar::new(conda_packages.len() as u64);
    bar.set_style(
        ProgressStyle::with_template(
            "[{elapsed_precise}] {bar:40.cyan/blue} {pos:>7}/{len:7} {msg}",
        )
        .expect("could not set progress style")
        .progress_chars("##-"),
    );

    let output_folder =
        tempfile::tempdir().map_err(|e| anyhow!("could not create temporary directory: {}", e))?;

    let channel_dir = output_folder.path().join(CHANNEL_DIRECTORY_NAME);

    stream::iter(conda_packages.iter())
        .map(Ok)
        .try_for_each_concurrent(50, |package| async {
            download_package(&client, package, &channel_dir).await?;

            bar.inc(1);

            Ok(())
        })
        .await
        .map_err(|e: anyhow::Error| anyhow!("could not download package: {}", e))?;

    bar.finish();

    // Create `repodata.json` files.
    create_repodata_files(conda_packages.iter(), &channel_dir).await?;

    // Add pixi-pack.json containing metadata.
    let metadata_path = output_folder.path().join(PIXI_PACK_METADATA_PATH);
    let mut metadata_file = File::create(&metadata_path).await?;

    let metadata = serde_json::to_string_pretty(&options.metadata)?;
    metadata_file.write_all(metadata.as_bytes()).await?;

    // Create environment file.
    create_environment_file(
        output_folder.path(),
        conda_packages.iter().map(|p| p.package_record()),
    )
    .await?;

    // Pack = archive + compress the contents.
    archive_directory(output_folder.path(), &options.output_file, options.level)
        .await
        .map_err(|e| anyhow!("could not archive directory: {}", e))?;

    // TODO: copy extra-files (parsed from pixi.toml), different compression algorithms, levels
    // todo: fail on pypi deps

    Ok(())
}

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
    let client = reqwest_middleware::ClientBuilder::new(
        reqwest::Client::builder()
            .no_gzip()
            .pool_max_idle_per_host(20)
            .user_agent("pixi-pack")
            .timeout(std::time::Duration::from_secs(timeout))
            .build()
            .map_err(|e| anyhow!("could not create download client: {}", e))?,
    )
    .with_arc(Arc::new(AuthenticationMiddleware::new(auth_storage)))
    .build();
    Ok(client)
}

/// Download a conda package to a given output directory.
async fn download_package(
    client: &ClientWithMiddleware,
    package: &CondaPackage,
    output_dir: &Path,
) -> Result<()> {
    let output_dir = output_dir.join(&package.package_record().subdir);
    create_dir_all(&output_dir)
        .await
        .map_err(|e| anyhow!("could not create download directory: {}", e))?;

    let file_name = package
        .file_name()
        .ok_or(anyhow!("could not get file name"))?;
    let mut dest = File::create(output_dir.join(file_name)).await?;

    tracing::debug!("Fetching package {}", package.url());
    let mut response = client.get(package.url().to_string()).send().await?;

    while let Some(chunk) = response.chunk().await? {
        dest.write_all(&chunk).await?;
    }

    Ok(())
}

/// Archive a directory into a compressed tarball.
async fn archive_directory(
    input_dir: &Path,
    archive_target: &Path,
    level: Option<Level>,
) -> Result<()> {
    let outfile = fs::File::create(archive_target).await.map_err(|e| {
        anyhow!(
            "could not create archive file at {}: {}",
            archive_target.display(),
            e
        )
    })?;

    let writer = tokio::io::BufWriter::new(outfile);

    let level = level.unwrap_or(Level::Default);
    let compressor = ZstdEncoder::with_quality(writer, level);

    let mut archive = Builder::new(compressor);

    archive
        .append_dir_all(".", input_dir)
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

async fn create_environment_file(
    destination: &Path,
    packages: impl IntoIterator<Item = &PackageRecord>,
) -> Result<()> {
    let environment_path = destination.join("environment.yml");

    let mut environment = String::new();

    environment.push_str("channels:\n");
    environment.push_str(&format!("  - ./{CHANNEL_DIRECTORY_NAME}\n",));
    environment.push_str("  - nodefaults\n");
    environment.push_str("dependencies:\n");

    for package in packages {
        let match_spec_str = format!(
            "{}={}={}",
            package.name.as_normalized(),
            package.version,
            package.build,
        );

        environment.push_str(&format!("  - {}\n", match_spec_str));
    }

    fs::write(environment_path, environment)
        .await
        .map_err(|e| anyhow!("Could not write environment file: {}", e))?;

    Ok(())
}

async fn create_repodata_files(
    packages: impl Iterator<Item = &CondaPackage>,
    channel_dir: &Path,
) -> Result<()> {
    let mut packages_per_subdir = HashMap::new();

    for p in packages {
        let package_record = p.package_record();
        let subdir = &package_record.subdir;

        let packages = packages_per_subdir.entry(subdir).or_insert_with(Vec::new);
        packages.push(p);
    }

    for (subdir, packages) in packages_per_subdir {
        let repodata_path = channel_dir.join(subdir).join("repodata.json");

        let conda_packages = packages
            .into_iter()
            .map(|p| {
                (
                    p.file_name()
                        .expect("Could not determine filename")
                        .to_string(),
                    p.package_record().clone(),
                )
            })
            .collect();

        let repodata = RepoData {
            info: Some(ChannelInfo {
                subdir: subdir.clone(),
                base_url: None,
            }),
            packages: HashMap::default(),
            conda_packages,
            removed: HashSet::default(),
            version: Some(2),
        };

        let repodata_json = serde_json::to_string_pretty(&repodata)
            .map_err(|e| anyhow!("could not serialize repodata: {}", e))?;
        fs::write(repodata_path, repodata_json)
            .map_err(|e| anyhow!("could not write repodata: {}", e))
            .await?;
    }

    Ok(())
}
