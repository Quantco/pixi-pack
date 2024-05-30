use std::{fs::{create_dir_all, File}, io::copy, path::{self, Path, PathBuf}, sync::Arc};

use clap::{Parser, Subcommand};
use futures::future::try_join_all;
use rattler_networking::{authentication_storage::{self, backends::file::{self, FileStorageError}}, AuthenticationMiddleware, AuthenticationStorage};
use rattler_package_streaming::fs::{extract_conda, extract_tar_bz2};
use reqwest_middleware::ClientWithMiddleware;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}


#[derive(Subcommand)]
enum Commands {
    /// Pack a pixi environment
    Pack {
        /// Environment to pack
        #[arg(short, long)]
        environment: String,

        /// Platform to pack
        #[arg(short, long)]
        platform: rattler_conda_types::Platform,

        /// Temporary output directory
        #[arg(short, long, default_value = "/tmp")]
        output_dir: PathBuf,

        #[arg(short, long)] // todo add env
        auth_file: Option<PathBuf>,
    },
    /// Unpack a pixi environment
    Unpack {
        // TODO
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let result = match &cli.command {
        Some(Commands::Pack { environment, platform, auth_file, output_dir }) => {
            pack(environment.clone(), platform.clone(), auth_file.clone(), output_dir.clone()).await
        }
        Some(Commands::Unpack {}) => {
            println!("Unpack environment");
            let target_dir = PathBuf::from("output-2");
            unpack(target_dir).await
        }
        None => {
            println!("No command specified");
            Ok(())
        }
    };
    result
}

pub fn get_auth_store(
    auth_file: Option<PathBuf>,
) -> Result<AuthenticationStorage, FileStorageError> {
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

fn reqwest_client_from_auth_storage(
    auth_file: Option<PathBuf>
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

async fn fetch_package(client: ClientWithMiddleware, package: rattler_lock::Package, output_dir: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let conda_package = package.as_conda().unwrap();
    let url = conda_package.url();
    tracing::info!("Fetching package: {}", url); // todo debug
    let response = client.get(url.to_string()).send().await?;
    let subdir = conda_package.package_record().subdir.clone();
    let output_dir = path::Path::new(&output_dir).join(subdir);
    create_dir_all(output_dir.clone())?;
    let file_name = url.path_segments().unwrap().last().unwrap();
    let mut dest = File::create(output_dir.clone().join(file_name))?;
    let content = response.bytes().await?;
    copy(&mut content.as_ref(), &mut dest)?;
    Ok(())
}

async fn pack(environment: String, platform: rattler_conda_types::Platform, auth_file: Option<PathBuf>, output_dir: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let lockfile = rattler_lock::LockFile::from_path(Path::new("test/pixi.lock")).unwrap();

    let client = reqwest_client_from_auth_storage(auth_file).unwrap();
    let env = lockfile.environment(&environment).unwrap();
    let packages = env.packages(platform).unwrap();

    // todo: progress bar
    try_join_all(
        packages.into_iter().map(|package| {
            fetch_package(client.clone(), package, output_dir.clone())
        })
    ).await?;

    rattler_index::index(output_dir.as_path(), None)?;

    // TODO: remove output directory
    // TODO: move to zstd, add progress bar, add pixi-pack.json, copy extra-files
    // different compression algorithms, levels

    Ok(())
}

fn collect_packages(channel: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let subdirs = channel.read_dir()?;
    let packages = subdirs.into_iter().flat_map(|subdir|{
        let subdir = subdir.unwrap().path();
        let packages = subdir.read_dir().unwrap();
        packages.into_iter().map(|package| {
            package.unwrap().path()
        }).collect::<Vec<PathBuf>>()
    }).collect();
    Ok(packages)
}

async fn unpack(target_dir: PathBuf) -> Result<(), Box<dyn std::error::Error>> {

    // todo: read conda packages directly from zstd

    let output_dir = Path::new("output");
    let packages = collect_packages(&output_dir).unwrap();
    let results = packages.into_iter().map(|package| {
        let file_extension = package.extension().unwrap();
        println!("{:?}", file_extension);
        let results = match file_extension.to_str().unwrap() {
            "bz2" => extract_tar_bz2(package.as_path(), &target_dir),
            "conda" => extract_conda(package.as_path(), &target_dir),
            // "json" => Ok(()),
            _ => panic!("Unsupported file extension")
        };
        results
    });
    Ok(())
}