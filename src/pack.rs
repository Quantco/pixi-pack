use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    time::Duration,
};

#[cfg(not(target_os = "windows"))]
use std::os::unix::fs::PermissionsExt as _;

use indicatif::HumanBytes;
use rattler_index::{package_record_from_conda, package_record_from_tar_bz2};
use tokio::{
    fs::{self, File, create_dir_all},
    io::AsyncWriteExt,
};

use anyhow::Result;
use base64::engine::{Engine, general_purpose::STANDARD};
use futures::{StreamExt, TryFutureExt, TryStreamExt, stream};
use rattler_conda_types::{ChannelInfo, PackageRecord, Platform, RepoData, package::ArchiveType};
use rattler_lock::{
    CondaBinaryData, CondaPackageData, LockFile, LockedPackageRef, PypiPackageData, UrlOrPath,
};
use rattler_networking::{
    AuthenticationMiddleware, AuthenticationStorage, MirrorMiddleware, S3Middleware,
    authentication_storage, mirror_middleware::Mirror,
};
use reqwest_middleware::ClientWithMiddleware;
use tar::{Builder, HeaderMode};
use tokio::io::AsyncReadExt;
use url::Url;
use uv_distribution_filename::WheelFilename;
use uv_distribution_types::RemoteSource;
use walkdir::WalkDir;

use crate::{
    CHANNEL_DIRECTORY_NAME, Config, PIXI_PACK_METADATA_PATH, PYPI_DIRECTORY_NAME, PixiPackMetadata,
    ProgressReporter, get_size,
};
use anyhow::anyhow;

static DEFAULT_REQWEST_TIMEOUT_SEC: Duration = Duration::from_secs(5 * 60);

/// Options for packing a pixi environment.
#[derive(Debug, Clone)]
pub struct PackOptions {
    pub environment: String,
    pub platform: Platform,
    pub auth_file: Option<PathBuf>,
    pub output_file: PathBuf,
    pub manifest_path: PathBuf,
    pub metadata: PixiPackMetadata,
    pub cache_dir: Option<PathBuf>,
    pub injected_packages: Vec<PathBuf>,
    pub ignore_pypi_non_wheel: bool,
    pub create_executable: bool,
    pub pixi_unpack_source: Option<UrlOrPath>,
    pub config: Option<Config>,
}

fn load_lockfile(manifest_path: &Path) -> Result<LockFile> {
    if !manifest_path.exists() {
        anyhow::bail!(
            "manifest path does not exist at {}",
            manifest_path.display()
        );
    }

    let manifest_path = if !manifest_path.is_dir() {
        manifest_path
            .parent()
            .ok_or(anyhow!("could not get parent directory"))?
    } else {
        manifest_path
    };

    let lockfile_path = manifest_path.join("pixi.lock");

    LockFile::from_path(&lockfile_path).map_err(|e| {
        anyhow!(
            "could not read lockfile at {}: {}",
            lockfile_path.display(),
            e
        )
    })
}

/// Pack a pixi environment.
pub async fn pack(options: PackOptions) -> Result<()> {
    let lockfile = load_lockfile(&options.manifest_path)?;

    let max_parallel_downloads = options.config.as_ref().map_or_else(
        rattler_config::config::concurrency::default_max_concurrent_downloads,
        |c| c.concurrency.downloads,
    );

    let client = reqwest_client_from_options(&options)
        .map_err(|e| anyhow!("could not create reqwest client from auth storage: {e}"))?;

    let env = lockfile.environment(&options.environment).ok_or(anyhow!(
        "environment not found in lockfile: {}",
        options.environment
    ))?;

    let packages = env.packages(options.platform).ok_or(anyhow!(
        "platform not found in lockfile: {}",
        options.platform.as_str()
    ))?;

    let output_folder =
        tempfile::tempdir().map_err(|e| anyhow!("could not create temporary directory: {}", e))?;

    let channel_dir = output_folder.path().join(CHANNEL_DIRECTORY_NAME);
    let pypi_directory = output_folder.path().join(PYPI_DIRECTORY_NAME);

    let mut conda_packages_from_lockfile: Vec<CondaBinaryData> = Vec::new();
    let mut pypi_packages_from_lockfile: Vec<PypiPackageData> = Vec::new();

    for package in packages {
        match package {
            LockedPackageRef::Conda(CondaPackageData::Binary(binary_data)) => {
                conda_packages_from_lockfile.push(binary_data.clone())
            }
            LockedPackageRef::Conda(CondaPackageData::Source(_)) => {
                anyhow::bail!("Conda source packages are not yet supported by pixi-pack")
            }
            LockedPackageRef::Pypi(pypi_data, _) => {
                let package_name = pypi_data.name.clone();
                let location = pypi_data.location.clone();
                let is_wheel = location
                    .file_name()
                    .filter(|x| x.ends_with("whl"))
                    .is_some();
                if is_wheel {
                    pypi_packages_from_lockfile.push(pypi_data.clone());
                } else if options.ignore_pypi_non_wheel {
                    tracing::warn!(
                        "ignoring PyPI package {} since it is not a wheel file",
                        package_name.to_string()
                    );
                } else {
                    anyhow::bail!(
                        "package {package_name} is not a wheel file, we currently require all dependencies to be wheels.",
                    );
                }
            }
        }
    }

    // Download packages to temporary directory.
    tracing::info!(
        "Downloading {} packages...",
        conda_packages_from_lockfile.len()
    );
    eprintln!(
        "⏳ Downloading {} packages...",
        conda_packages_from_lockfile.len()
    );
    let bar = ProgressReporter::new(conda_packages_from_lockfile.len() as u64);
    stream::iter(conda_packages_from_lockfile.iter())
        .map(Ok)
        .try_for_each_concurrent(max_parallel_downloads, |package| async {
            download_package(&client, package, &channel_dir, options.cache_dir.as_deref()).await?;
            bar.pb.inc(1);
            Ok(())
        })
        .await
        .map_err(|e: anyhow::Error| anyhow!("could not download package: {}", e))?;
    bar.pb.finish_and_clear();

    let mut conda_packages: Vec<(String, PackageRecord)> = Vec::new();

    for package in conda_packages_from_lockfile {
        let filename = package.file_name;
        conda_packages.push((filename, package.package_record));
    }

    let injected_packages: Vec<(PathBuf, ArchiveType)> = options
        .injected_packages
        .iter()
        .filter_map(|e| {
            ArchiveType::split_str(e.as_path().to_string_lossy().as_ref())
                .map(|(p, t)| (PathBuf::from(format!("{}{}", p, t.extension())), t))
        })
        .collect();

    tracing::info!("Injecting {} packages", injected_packages.len());
    for (path, archive_type) in injected_packages.iter() {
        // step 1: Derive PackageRecord from index.json inside the package
        let package_record = match archive_type {
            ArchiveType::TarBz2 => package_record_from_tar_bz2(path),
            ArchiveType::Conda => package_record_from_conda(path),
        }?;

        // step 2: Copy file into channel dir
        let subdir = &package_record.subdir;
        let filename = path
            .file_name()
            .ok_or(anyhow!("could not get file name"))?
            .to_str()
            .ok_or(anyhow!("could not convert filename to string"))?
            .to_string();

        fs::copy(&path, channel_dir.join(subdir).join(&filename))
            .await
            .map_err(|e| anyhow!("could not copy file to channel directory: {}", e))?;

        conda_packages.push((filename, package_record));
    }

    // In case we injected packages, we need to validate that these packages are solvable with the
    // environment (i.e., that each packages dependencies and run constraints are still satisfied).
    if !injected_packages.is_empty() {
        PackageRecord::validate(conda_packages.iter().map(|(_, p)| p.clone()).collect())?;
    }

    if !pypi_packages_from_lockfile.is_empty() {
        // Download pypi packages.
        tracing::info!(
            "Downloading {} pypi packages...",
            pypi_packages_from_lockfile.len()
        );
        eprintln!(
            "⏳ Downloading {} pypi packages...",
            pypi_packages_from_lockfile.len()
        );
        let bar = ProgressReporter::new(pypi_packages_from_lockfile.len() as u64);
        stream::iter(pypi_packages_from_lockfile.iter())
            .map(Ok)
            .try_for_each_concurrent(max_parallel_downloads, |package: &PypiPackageData| async {
                download_pypi_package(
                    &client,
                    package,
                    &pypi_directory,
                    options.cache_dir.as_deref(),
                )
                .await?;
                bar.pb.inc(1);
                Ok(())
            })
            .await
            .map_err(|e: anyhow::Error| anyhow!("could not download pypi package: {}", e))?;
        bar.pb.finish_and_clear();
    }

    let injected_pypi_packages: Vec<_> = options
        .injected_packages
        .iter()
        .filter(|e| {
            e.extension()
                .filter(|e| e.to_str() == Some("whl"))
                .is_some()
        })
        .cloned()
        .collect();

    tracing::info!("Injecting {} pypi packages", injected_pypi_packages.len());
    for path in injected_pypi_packages {
        let filename = path
            .file_name()
            .ok_or(anyhow!("could not get filename"))?
            .to_str()
            .ok_or(anyhow!("could not convert filename to string"))?
            .to_string();
        let path_str = path
            .to_str()
            .ok_or(anyhow!("could not convert filename to string"))?
            .to_string();
        let wheel_file_name = WheelFilename::from_str(&filename)?;
        let pypi_data = PypiPackageData {
            name: wheel_file_name
                .name
                .as_str()
                .parse()
                .map_err(|e| anyhow!("could not parse package name: {}", e))?,
            version: wheel_file_name
                .version
                .to_string()
                .parse()
                .map_err(|e| anyhow!("could not parse package version: {}", e))?,
            location: path_str
                .parse()
                .map_err(|e| anyhow!("could not convert path type: {}", e))?,
            hash: None,
            requires_dist: vec![],
            requires_python: None,
            editable: false,
        };
        create_dir_all(&pypi_directory)
            .await
            .map_err(|e| anyhow!("could not create pypi directory: {}", e))?;
        tracing::warn!(
            "Currently we cannot verify that injected wheels are compatible with the environment."
        );
        fs::copy(&path, pypi_directory.join(filename)).await?;

        pypi_packages_from_lockfile.push(pypi_data.clone());
    }

    // Create `repodata.json` files.
    tracing::info!("Creating repodata.json files");
    create_repodata_files(conda_packages.iter(), &channel_dir).await?;

    // Add pixi-pack.json containing metadata.
    tracing::info!("Creating pixi-pack.json file");
    let metadata_path = output_folder.path().join(PIXI_PACK_METADATA_PATH);
    let metadata = serde_json::to_string_pretty(&options.metadata)?;
    fs::write(metadata_path, metadata.as_bytes()).await?;

    // Create environment file.
    tracing::info!("Creating environment.yml file");
    create_environment_file(
        output_folder.path(),
        conda_packages.iter().map(|(_, p)| p),
        &pypi_packages_from_lockfile,
    )
    .await?;

    // Pack = archive the contents.
    tracing::info!("Creating pack at {}", options.output_file.display());
    archive_directory(
        output_folder.path(),
        &options.output_file,
        options.create_executable,
        options.pixi_unpack_source,
        options.platform,
    )
    .await
    .map_err(|e| anyhow!("could not archive directory: {}", e))?;

    let output_size = HumanBytes(get_size(&options.output_file)?).to_string();
    tracing::info!(
        "Created pack at {} with size {}.",
        options.output_file.display(),
        output_size
    );
    eprintln!(
        "📦 Created pack at {} with size {}.",
        options.output_file.display(),
        output_size
    );

    Ok(())
}

/// Get the authentication storage from the given auth file path.
fn get_auth_store(auth_file: Option<PathBuf>) -> Result<AuthenticationStorage> {
    let mut store = AuthenticationStorage::from_env_and_defaults()?;
    if let Some(auth_file) = auth_file {
        tracing::info!("Loading authentication from file: {:?}", auth_file);

        if !auth_file.exists() {
            return Err(anyhow::anyhow!(
                "Authentication file does not exist: {:?}",
                auth_file
            ));
        }

        store.backends.insert(
            0,
            Arc::from(
                authentication_storage::backends::file::FileStorage::from_path(PathBuf::from(
                    &auth_file,
                ))?,
            ),
        );
    }
    Ok(store)
}

/// Create a reqwest client (optionally including authentication middleware).
fn reqwest_client_from_options(options: &PackOptions) -> Result<ClientWithMiddleware> {
    let auth_storage = get_auth_store(options.auth_file.clone())?;

    let s3_middleware = if let Some(config) = &options.config {
        let s3_config = rattler_networking::s3_middleware::compute_s3_config(&config.s3_options.0);
        tracing::info!("Using S3 config: {:?}", s3_config);
        S3Middleware::new(s3_config, auth_storage.clone())
    } else {
        S3Middleware::new(HashMap::new(), auth_storage.clone())
    };
    let mirror_middleware = if let Some(config) = &options.config {
        let mut internal_map = HashMap::new();
        tracing::info!("Using mirrors: {:?}", config.mirrors);

        fn ensure_trailing_slash(url: &url::Url) -> url::Url {
            if url.path().ends_with('/') {
                url.clone()
            } else {
                // Do not use `join` because it removes the last element
                format!("{}/", url)
                    .parse()
                    .expect("Failed to add trailing slash to URL")
            }
        }
        for (key, value) in &config.mirrors {
            let mut mirrors = Vec::new();
            for v in value {
                mirrors.push(Mirror {
                    url: ensure_trailing_slash(v),
                    no_jlap: false,
                    no_bz2: false,
                    no_zstd: false,
                    max_failures: None,
                });
            }
            internal_map.insert(ensure_trailing_slash(key), mirrors);
        }
        MirrorMiddleware::from_map(internal_map)
    } else {
        MirrorMiddleware::from_map(HashMap::new())
    };

    let client = reqwest_middleware::ClientBuilder::new(
        reqwest::Client::builder()
            .no_gzip()
            .pool_max_idle_per_host(20)
            .user_agent(format!("pixi-pack/{}", env!("CARGO_PKG_VERSION")))
            .read_timeout(DEFAULT_REQWEST_TIMEOUT_SEC)
            .build()
            .map_err(|e| anyhow!("could not create download client: {}", e))?,
    )
    .with(mirror_middleware)
    .with(s3_middleware)
    .with_arc(Arc::new(AuthenticationMiddleware::from_auth_storage(
        auth_storage,
    )))
    .build();
    Ok(client)
}

/// Download a conda package to a given output directory.
async fn download_package(
    client: &ClientWithMiddleware,
    package: &CondaBinaryData,
    output_dir: &Path,
    cache_dir: Option<&Path>,
) -> Result<()> {
    let output_dir = output_dir.join(&package.package_record.subdir);
    create_dir_all(&output_dir)
        .await
        .map_err(|e| anyhow!("could not create download directory: {}", e))?;

    let file_name = &package.file_name;
    let output_path = output_dir.join(file_name);

    // Check cache first if enabled
    if let Some(cache_dir) = cache_dir {
        let cache_path = cache_dir
            .join(&package.package_record.subdir)
            .join(file_name);
        if cache_path.exists() {
            tracing::debug!("Using cached package from {}", cache_path.display());
            fs::copy(&cache_path, &output_path).await?;
            return Ok(());
        }
    }

    let url = package.location.try_into_url()?;
    match url.scheme() {
        "file" => {
            let local_path = url
                .to_file_path()
                .map_err(|_| anyhow!("could not convert url: {} to file path", url))?;
            tracing::debug!("Copying from path: {}", local_path.display());
            // Copy file
            fs::copy(local_path, &output_path).await?;
        }
        _ => {
            let mut dest = File::create(&output_path).await?;

            tracing::debug!("Fetching package {}", package.location);
            let mut response = client.get(url.clone()).send().await?.error_for_status()?;
            while let Some(chunk) = response.chunk().await? {
                dest.write_all(&chunk).await?;
            }
        }
    }

    // Save to cache if enabled
    if let Some(cache_dir) = cache_dir {
        let cache_subdir = cache_dir.join(&package.package_record.subdir);
        create_dir_all(&cache_subdir).await?;
        let cache_path = cache_subdir.join(file_name);
        fs::copy(&output_path, &cache_path).await?;
    }

    Ok(())
}
async fn archive_directory(
    input_dir: &Path,
    archive_target: &Path,
    create_executable: bool,
    pixi_unpack_source: Option<UrlOrPath>,
    platform: Platform,
) -> Result<()> {
    if create_executable {
        eprintln!("📦 Creating self-extracting executable");
        create_self_extracting_executable(input_dir, archive_target, pixi_unpack_source, platform)
            .await
    } else {
        create_tarball(input_dir, archive_target)
    }
}

fn write_archive<T>(mut archive: Builder<T>, input_dir: &Path) -> Result<()>
where
    T: std::io::Write + Unpin + Send,
{
    archive.mode(HeaderMode::Deterministic);
    // need to sort files to ensure deterministic output
    let files = WalkDir::new(input_dir)
        .sort_by_file_name()
        .into_iter()
        .collect::<Result<Vec<_>, walkdir::Error>>()
        .map_err(|e| anyhow!("could not walk directory: {}", e))?;
    for file in files {
        let path = file.path();
        let relative_path = path
            .strip_prefix(input_dir)
            .map_err(|e| anyhow!("could not strip prefix: {}", e))?;
        if relative_path == Path::new("") {
            continue;
        }
        if path.is_dir() {
            archive.append_dir(relative_path, input_dir)?;
        } else {
            archive.append_path_with_name(path, relative_path)?;
        }
    }

    let mut compressor = archive
        .into_inner()
        .map_err(|e| anyhow!("could not finish writing archive: {}", e))?;

    compressor
        .flush()
        .map_err(|e| anyhow!("could not flush output: {}", e))?;

    Ok(())
}

fn create_tarball(input_dir: &Path, archive_target: &Path) -> Result<()> {
    let outfile = std::fs::File::create(archive_target).map_err(|e| {
        anyhow!(
            "could not create archive file at {}: {}",
            archive_target.display(),
            e
        )
    })?;

    let writer = std::io::BufWriter::new(outfile);
    let archive = Builder::new(writer);

    write_archive(archive, input_dir)?;

    Ok(())
}

async fn download_pixi_unpack_executable(
    pixi_pack_source: Option<UrlOrPath>,
    platform: Platform,
) -> Result<Vec<u8>> {
    let (os, arch) = match platform {
        Platform::Linux64 => ("unknown-linux-musl", "x86_64"),
        Platform::LinuxAarch64 => ("unknown-linux-musl", "aarch64"),
        Platform::Osx64 => ("apple-darwin", "x86_64"),
        Platform::OsxArm64 => ("apple-darwin", "aarch64"),
        Platform::Win64 => ("pc-windows-msvc", "x86_64"),
        Platform::WinArm64 => ("pc-windows-msvc", "aarch64"),
        _ => return Err(anyhow!("Unsupported platform: {}", platform)),
    };
    let executable_name = format!("pixi-unpack-{}-{}", arch, os);
    let extension = if platform.is_windows() { ".exe" } else { "" };
    let version = env!("CARGO_PKG_VERSION");

    // Build pixi-unpack executable url
    let url = pixi_pack_source.unwrap_or_else(|| {
        let default_url = format!(
            "https://github.com/Quantco/pixi-pack/releases/download/v{}/{}{}",
            version, executable_name, extension
        );
        UrlOrPath::Url(default_url.parse().expect("could not parse url"))
    });

    eprintln!("📥 Fetching pixi-unpack executable...");

    let mut executable_bytes = Vec::new();

    // Use reqwest to download the pixi-unpack executable from the URL
    // or read it from a local file if the URL is a file path
    if let UrlOrPath::Url(_) = &url {
        let client = reqwest::Client::new();
        let response = client.get(url.to_string()).send().await?;
        if !response.status().is_success() {
            return Err(anyhow!(
                "Failed to download pixi-unpack executable from {}. Status: {}",
                url,
                response.status()
            ));
        }

        let total_size = response
            .content_length()
            .ok_or_else(|| anyhow!("Failed to get content length"))?;

        let bar = ProgressReporter::new(total_size);
        bar.pb.set_message("Downloading");

        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            executable_bytes.extend_from_slice(&chunk);
            bar.pb.inc(chunk.len() as u64);
        }

        bar.pb.finish_with_message("Download complete");
    } else {
        let mut file = File::open(url.to_string())
            .await
            .map_err(|e| anyhow!("Failed to open local file {}: {}", url, e))?;
        file.read_to_end(&mut executable_bytes)
            .await
            .map_err(|e| anyhow!("Failed to read local file {}: {}", url, e))?;
    }

    eprintln!("✅ pixi-unpack executable downloaded successfully");

    Ok(executable_bytes)
}

async fn create_self_extracting_executable(
    input_dir: &Path,
    target: &Path,
    pixi_pack_source: Option<UrlOrPath>,
    platform: Platform,
) -> Result<()> {
    let line_ending = if platform.is_windows() {
        b"\r\n".to_vec()
    } else {
        b"\n".to_vec()
    };

    // Set target executable path
    let executable_path = target.with_extension(if platform.is_windows() { "ps1" } else { "sh" });
    let mut final_executable = std::fs::File::create(&executable_path)
        .map_err(|e| anyhow!("could not create final executable file: {}", e))?;

    // Write header
    let windows_header = include_str!("header.ps1");
    let unix_header = include_str!("header.sh");
    let header = if platform.is_windows() {
        windows_header
    } else {
        unix_header
    };
    final_executable.write_all(header.as_bytes())?;
    final_executable.write_all(&line_ending)?; // Add a newline after the header

    // Write archive containing environment
    let writer =
        base64::write::EncoderWriter::new(std::io::BufWriter::new(&final_executable), &STANDARD);
    let archive = Builder::new(writer);
    write_archive(archive, input_dir)?;
    final_executable.write_all(&line_ending)?;

    // Write footer
    if platform.is_windows() {
        final_executable.write_all(b"__END_ARCHIVE__")?;
    } else {
        final_executable.write_all(b"@@END_ARCHIVE@@")?;
    }
    final_executable.write_all(&line_ending)?;

    // Write pixi-unpack executable bytes
    let executable_bytes = download_pixi_unpack_executable(pixi_pack_source, platform).await?;
    // Encode the executable to base64
    let executable_base64 = STANDARD.encode(&executable_bytes);
    final_executable.write_all(executable_base64.as_bytes())?;

    // Make the script executable
    // This won't be executed when cross-packing due to Windows FS not supporting Unix permissions
    #[cfg(not(target_os = "windows"))]
    if !platform.is_windows() {
        let mut perms = final_executable.metadata()?.permissions();
        perms.set_mode(0o755);
        final_executable.set_permissions(perms)?;
    }

    Ok(())
}

/// Create an `environment.yml` file from the given packages.
async fn create_environment_file(
    destination: &Path,
    packages: impl IntoIterator<Item = &PackageRecord>,
    pypi_packages: &Vec<PypiPackageData>,
) -> Result<()> {
    let environment_path = destination.join("environment.yml");

    let mut environment = String::new();

    environment.push_str("channels:\n");
    environment.push_str(&format!("  - ./{CHANNEL_DIRECTORY_NAME}\n",));
    environment.push_str("  - nodefaults\n");
    environment.push_str("dependencies:\n");

    let mut has_pip = false;
    for package in packages {
        let match_spec_str = format!(
            "{}={}={}",
            package.name.as_normalized(),
            package.version,
            package.build,
        );

        environment.push_str(&format!("  - {}\n", match_spec_str));

        if package.name.as_normalized() == "pip" {
            has_pip = true;
        }
    }

    if !pypi_packages.is_empty() {
        if !has_pip {
            tracing::warn!("conda/micromamba compatibility mode cannot work if no pip installed.");
        }

        environment.push_str("  - pip:\n");
        environment.push_str("    - --no-index\n");
        environment.push_str(&format!("    - --find-links ./{PYPI_DIRECTORY_NAME}\n"));

        for p in pypi_packages {
            environment.push_str(&format!("    - {}=={}\n", p.name, p.version));
        }
    }

    fs::write(environment_path.as_path(), environment)
        .await
        .map_err(|e| anyhow!("Could not write environment file: {}", e))?;

    Ok(())
}

/// Create `repodata.json` files for the given packages.
async fn create_repodata_files(
    packages: impl Iterator<Item = &(String, PackageRecord)>,
    channel_dir: &Path,
) -> Result<()> {
    let mut packages_per_subdir = HashMap::new();

    for (filename, p) in packages {
        let subdir = &p.subdir;

        let packages = packages_per_subdir
            .entry(subdir)
            .or_insert_with(HashMap::new);
        packages.insert(filename, p);
    }

    for (subdir, packages) in packages_per_subdir {
        let repodata_path = channel_dir.join(subdir).join("repodata.json");

        let conda_packages = packages
            .into_iter()
            .map(|(filename, p)| (filename.to_string(), p.clone()))
            .collect();

        let repodata = RepoData {
            info: Some(ChannelInfo {
                subdir: Some(subdir.clone()),
                base_url: None,
            }),
            packages: Default::default(),
            conda_packages,
            removed: Default::default(),
            version: Some(2),
        };

        let repodata_json = serde_json::to_string_pretty(&repodata)
            .map_err(|e| anyhow!("could not serialize repodata: {}", e))?;
        fs::write(repodata_path.as_path(), repodata_json)
            .map_err(|e| anyhow!("could not write repodata: {}", e))
            .await?;
    }

    Ok(())
}

/// Download a pypi package to a given output directory
async fn download_pypi_package(
    client: &ClientWithMiddleware,
    package: &PypiPackageData,
    output_dir: &Path,
    cache_dir: Option<&Path>,
) -> Result<()> {
    create_dir_all(output_dir)
        .await
        .map_err(|e| anyhow!("could not create download directory: {}", e))?;

    let url = match &package.location {
        UrlOrPath::Url(url) => url
            .as_ref()
            .strip_prefix("direct+")
            .and_then(|str| Url::parse(str).ok())
            .unwrap_or(url.clone()),
        UrlOrPath::Path(path) => anyhow::bail!("Path not supported: {}", path),
    };

    // Use `RemoteSource::filename()` from `uv_distribution_types` to decode filename
    // Because it may be percent-encoded
    let file_name = url.filename()?.to_string();
    let output_path = output_dir.join(&file_name);

    if let Some(cache_dir) = cache_dir {
        let cache_path = cache_dir.join(PYPI_DIRECTORY_NAME).join(&file_name);
        if cache_path.exists() {
            tracing::debug!("Using cached package from {}", cache_path.display());
            fs::copy(&cache_path, &output_path).await?;
            return Ok(());
        }
    }

    let mut dest = File::create(&output_path).await?;
    tracing::debug!("Fetching package {}", url);

    let mut response = client.get(url.clone()).send().await?.error_for_status()?;

    while let Some(chunk) = response.chunk().await? {
        dest.write_all(&chunk).await?;
    }

    if let Some(cache_dir) = cache_dir {
        let cache_subdir = cache_dir.join(PYPI_DIRECTORY_NAME);
        create_dir_all(&cache_subdir).await?;
        let cache_path = cache_subdir.join(&file_name);
        fs::copy(&output_path, &cache_path).await?;
    }

    Ok(())
}
