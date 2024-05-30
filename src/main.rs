use std::path::PathBuf;

use clap::{Parser, Subcommand};
use rattler_conda_types::Platform;

use pixi_pack::{pack, unpack, PackOptions, PixiPackMetadata, UnpackOptions};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;

/* -------------------------------------------- CLI -------------------------------------------- */

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap()
}

/// The pixi-pack CLI.
#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Working directory (i.e., where the pixi.toml/pixi.lock files are located)
    #[arg(short, long, default_value = cwd().into_os_string())]
    working_directory: PathBuf,

    /// Output (pack) or target (unpack) directory
    #[arg(short, long, default_value = cwd().into_os_string())]
    output_dir: PathBuf,

    /// The pack format version
    #[arg(short, long, default_value = "v1")]
    pack_version: String,
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
    },

    /// Unpack a pixi environment
    Unpack {
        /// Input file ("pack")
        #[arg(short, long)]
        pack_file: PathBuf,
    },
}

/* -------------------------------------------- MAIN ------------------------------------------- */

/// The main entrypoint for the pixi-pack CLI.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    tracing::info!("Starting pixi-pack CLI");

    let cli = Cli::parse();
    let result = match &cli.command {
        Some(Commands::Pack {
            environment,
            platform,
            auth_file,
        }) => {
            let options = PackOptions {
                environment: environment.clone(),
                platform: platform.clone(),
                auth_file: auth_file.clone(),
                output_dir: cli.output_dir.clone(),
                input_dir: cli.working_directory.clone(),
                metadata: PixiPackMetadata {
                    version: cli.pack_version.clone(),
                },
            };
            tracing::info!("Running pack command with options: {:?}", options);
            pack(options).await
        }
        Some(Commands::Unpack { pack_file }) => {
            let options = UnpackOptions {
                pack_file: pack_file.clone(),
                target_dir: cli.output_dir.clone(),
            };
            tracing::info!("Running unpack command with options: {:?}", options);
            unpack(options).await
        }
        None => {
            panic!("No subcommand provided")
        }
    };
    tracing::info!("Finished running pixi-pack");
    result
}
