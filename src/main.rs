use std::path::PathBuf;

use clap::{Parser, Subcommand};
use rattler_conda_types::Platform;

use pixi_pack::{pack, unpack, PackOptions, PixiPackMetadata, UnpackOptions, DEFAULT_PIXI_PACK_VERSION};
use rattler_shell::shell::ShellEnum;
use anyhow::Result;

/* -------------------------------------------- CLI -------------------------------------------- */

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap()
}

/// The pixi-pack CLI.
#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// The subcommands for the pixi-pack CLI.
#[derive(Subcommand)]
enum Commands {
    /// Pack a pixi environment
    Pack {
        /// Environment to pack
        #[arg(short, long, default_value = "default")]
        environment: String,

        /// Platform to pack
        #[arg(short, long, default_value = Platform::current().as_str())]
        platform: Platform,

        /// Authentication file for fetching packages
        #[arg(short, long)] // TODO: Read from environment variable?
        auth_file: Option<PathBuf>,

        /// The path to 'pixi.toml' or 'pyproject.toml'
        #[arg(required = true)]
        manifest_path: PathBuf,

        /// Output file to write the pack to
        #[arg(short, long, default_value = cwd().join("environment.tar.zstd").into_os_string())]
        output_file: PathBuf,
    },

    /// Unpack a pixi environment
    Unpack {
        /// Where to unpack the environment.
        /// The environment will be unpacked into a `env` subdirectory of this path.
        /// The activation script will be written to the root of this path.
        #[arg(short, long, default_value = cwd().into_os_string())]
        output_directory: PathBuf,

        /// Path to the pack file
        #[arg()]
        pack_file: PathBuf,

        /// Sets the shell, options: [`bash`, `zsh`, `xonsh`, `cmd`, `powershell`, `fish`, `nushell`]
        #[arg(short, long)]
        shell: Option<ShellEnum>
    },
}

/* -------------------------------------------- MAIN ------------------------------------------- */

/// The main entrypoint for the pixi-pack CLI.
#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    tracing::debug!("Starting pixi-pack CLI");

    let cli = Cli::parse();
    let result = match cli.command {
        Commands::Pack {
            environment,
            platform,
            auth_file,
            manifest_path,
            output_file,
        } => {
            let options = PackOptions {
                environment,
                platform,
                auth_file,
                output_file,
                manifest_path,
                metadata: PixiPackMetadata {
                    version: DEFAULT_PIXI_PACK_VERSION.to_string(),
                    platform,
                },
                level: None,
            };
            tracing::debug!("Running pack command with options: {:?}", options);
            pack(options).await
        }
        Commands::Unpack { output_directory, pack_file, shell } => {
            let options = UnpackOptions {
                pack_file,
                output_directory,
                shell
            };
            tracing::debug!("Running unpack command with options: {:?}", options);
            unpack(options).await
        }
    };
    tracing::debug!("Finished running pixi-pack");
    result
}
