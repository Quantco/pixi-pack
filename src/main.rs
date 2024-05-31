use std::path::PathBuf;

use clap::{Parser, Subcommand};
use rattler_conda_types::Platform;

use pixi_pack::{pack, unpack, PackOptions, PixiPackMetadata, UnpackOptions};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;

/* -------------------------------------------- CLI -------------------------------------------- */

const DEFAULT_PACK_VERSION: &str = "1";

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap()
}

/// The pixi-pack CLI.
#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

/// The subcommands for the pixi-pack CLI.
#[derive(Subcommand)]
enum Commands {
    /// Pack a pixi environment
    Pack {
        /// Environment to pack
        #[arg(short, long)]
        environment: String,

        /// Platform to pack
        #[arg(short, long)]
        platform: Platform,

        /// Authentication file for fetching packages
        #[arg(short, long)] // TODO: Read from environment variable?
        auth_file: Option<PathBuf>,

        /// The path to 'pixi.toml' or 'pyproject.toml'
        #[arg(short, long, default_value = cwd().join("pixi.toml").into_os_string())]
        manifest_path: PathBuf,

        /// Output file to write the pack to
        #[arg(short, long, default_value = cwd().join("environment.tar.zstd").into_os_string())]
        output_file: PathBuf,
    },

    /// Unpack a pixi environment
    Unpack {
        /// Prefix to unpack to
        #[arg(short, long, default_value = cwd().join("env").into_os_string())]
        prefix: PathBuf,

        /// Path to the pack file
        #[arg()]
        pack_file: PathBuf,
    },
}

/* -------------------------------------------- MAIN ------------------------------------------- */

/// The main entrypoint for the pixi-pack CLI.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    tracing::debug!("Starting pixi-pack CLI");

    let cli = Cli::parse();
    let result = match &cli.command {
        Some(Commands::Pack {
            environment,
            platform,
            auth_file,
            manifest_path,
            output_file,
        }) => {
            let options = PackOptions {
                environment: environment.clone(),
                platform: platform.clone(),
                auth_file: auth_file.clone(),
                output_file: output_file.clone(),
                manifest_path: manifest_path.clone(),
                metadata: PixiPackMetadata {
                    version: DEFAULT_PACK_VERSION.to_string(),
                },
            };
            tracing::debug!("Running pack command with options: {:?}", options);
            pack(options).await
        }
        Some(Commands::Unpack { prefix, pack_file }) => {
            let options = UnpackOptions {
                pack_file: pack_file.clone(),
                prefix: prefix.clone(),
            };
            tracing::debug!("Running unpack command with options: {:?}", options);
            unpack(options).await
        }
        None => {
            panic!("No subcommand provided")
        }
    };
    tracing::debug!("Finished running pixi-pack");
    result
}
