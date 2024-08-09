use std::path::PathBuf;

use clap::{Parser, Subcommand};
use clap_verbosity_flag::Verbosity;
use rattler_conda_types::Platform;

use anyhow::Result;
use pixi_pack::{
    pack, unpack, PackOptions, PixiPackMetadata, UnpackOptions, DEFAULT_PIXI_PACK_VERSION,
};
use rattler_shell::shell::ShellEnum;
use tracing_log::AsTrace;

/* -------------------------------------------- CLI -------------------------------------------- */

fn cwd() -> PathBuf {
    std::env::current_dir().expect("failed to obtain current working directory")
}

/// The pixi-pack CLI.
#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[command(flatten)]
    verbose: Verbosity,
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
        #[arg(long)] // TODO: Read from environment variable?
        auth_file: Option<PathBuf>,

        /// The path to 'pixi.toml' or 'pyproject.toml'
        #[arg(default_value = cwd().join("pixi.toml").into_os_string())]
        manifest_path: PathBuf,

        /// Output file to write the pack to (will be an archive)
        #[arg(short, long, default_value = cwd().join("environment").into_os_string())]
        output_file: PathBuf,

        /// Inject an additional conda package into the final prefix
        #[arg(short, long, num_args(0..))]
        inject: Vec<PathBuf>,

        /// PyPI dependencies are not supported.
        /// This flag allows packing even if PyPI dependencies are present.
        #[arg(long, default_value = "false")]
        ignore_pypi_errors: bool,

        /// Create self-extracting executable
        /// This feature is only available on macOS and Linux.
        #[arg(long, default_value = "false")]
        create_executable: bool,
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
async fn main() -> Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(cli.verbose.log_level_filter().as_trace())
        .init();

    tracing::debug!("Starting pixi-pack CLI");

    match cli.command {
        Commands::Pack {
            environment,
            platform,
            auth_file,
            manifest_path,
            output_file,
            inject,
            ignore_pypi_errors,
            create_executable,
        } => {
            if create_executable && is_unsupported_platform(&platform) {
                return Err(anyhow::anyhow!("Creating self-extracting executables is only supported on macOS and Linux. Current platform: {}", platform));
            }

            let mut output_file_with_extension = output_file;

            if create_executable {
                // TODO: Add support for other platforms
                // Change this to shell.extension()
                output_file_with_extension = output_file_with_extension.with_extension("sh");
            } else {
                output_file_with_extension = output_file_with_extension.with_extension("tar");
            }

            let options = PackOptions {
                environment,
                platform,
                auth_file,
                output_file: output_file_with_extension,
                manifest_path,
                metadata: PixiPackMetadata {
                    version: DEFAULT_PIXI_PACK_VERSION.to_string(),
                    platform,
                },
                injected_packages: inject,
                ignore_pypi_errors,
                create_executable,
            };
            tracing::debug!("Running pack command with options: {:?}", options);
            pack(options).await?
        }
        Commands::Unpack {
            output_directory,
            pack_file,
            shell,
        } => {
            let options = UnpackOptions {
                pack_file,
                output_directory,
                shell,
            };
            tracing::debug!("Running unpack command with options: {:?}", options);
            unpack(options).await?
        }
    };
    tracing::debug!("Finished running pixi-pack");

    Ok(())
}

/// Check if the given platform supports creating self-extracting executables
fn is_unsupported_platform(platform: &Platform) -> bool {
    matches!(
        platform,
        Platform::Win32 | Platform::Win64 | Platform::WinArm64
    )
}
