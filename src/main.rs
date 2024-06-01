use core::fmt;
use derive_more::From;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use rattler_conda_types::Platform;

use pixi_pack::{
    pack, unpack, PackError, PackOptions, PixiPackMetadata, UnpackError, UnpackOptions,
    DEFAULT_PIXI_PACK_VERSION,
};
use rattler_shell::shell::ShellEnum;

/* -------------------------------------------- CLI -------------------------------------------- */

fn cwd() -> PathBuf {
    std::env::current_dir().expect("todo: error handling")
}

#[derive(Debug, From)]
pub enum PixiPackError {
    NoSubcommandProvided,
    UnpackError(UnpackError),
    PackError(PackError),
}
impl std::error::Error for PixiPackError {}
impl fmt::Display for PixiPackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PixiPackError::UnpackError(e) => e.fmt(f),
            PixiPackError::PackError(e) => e.fmt(f),
            PixiPackError::NoSubcommandProvided => write!(f, "No subcommand provided"),
        }
    }
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
        #[arg(short, long, default_value = "default")]
        environment: String,

        /// Platform to pack
        #[arg(short, long, default_value = Platform::current().as_str())]
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
        shell: Option<ShellEnum>,
    },
}

/* -------------------------------------------- MAIN ------------------------------------------- */

/// The main entrypoint for the pixi-pack CLI.
#[tokio::main]
async fn main() {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber).expect("TODO: error handling");

    tracing::debug!("Starting pixi-pack CLI");

    let cli = Cli::parse();
    let result: Result<(), PixiPackError> = match &cli.command {
        Some(Commands::Pack {
            environment,
            platform,
            auth_file,
            manifest_path,
            output_file,
        }) => {
            let options = PackOptions {
                environment: environment.clone(),
                platform: *platform,
                auth_file: auth_file.clone(),
                output_file: output_file.clone(),
                manifest_path: manifest_path.clone(),
                metadata: PixiPackMetadata {
                    version: DEFAULT_PIXI_PACK_VERSION.to_string(),
                    platform: *platform,
                },
            };
            tracing::debug!("Running pack command with options: {:?}", options);
            pack(options).await.map_err(|e| e.into())
        }
        Some(Commands::Unpack {
            output_directory,
            pack_file,
            shell,
        }) => {
            let options = UnpackOptions {
                pack_file: pack_file.clone(),
                output_directory: output_directory.clone(),
                shell: shell.clone(),
            };
            tracing::debug!("Running unpack command with options: {:?}", options);
            unpack(options).await.map_err(|e| e.into())
        }
        None => {
            Err(PixiPackError::NoSubcommandProvided)
        }
    };
    tracing::debug!("Finished running pixi-pack");
    if let Err(e) = result {
        tracing::error!("{}", e);
        std::process::exit(1);
    }
}
