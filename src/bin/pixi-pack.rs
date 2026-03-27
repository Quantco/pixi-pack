use std::io;
use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use clap_verbosity_flag::Verbosity;
use rattler_conda_types::Platform;

use anyhow::Result;
use pixi_pack::{
    Config, DEFAULT_PIXI_PACK_VERSION, OutputMode, PIXI_PACK_VERSION, PackOptions,
    PixiPackMetadata, pack,
};
use rattler_lock::UrlOrPath;

/* -------------------------------------------- CLI -------------------------------------------- */

fn cwd() -> PathBuf {
    std::env::current_dir().expect("failed to obtain current working directory")
}

/// The pixi-pack CLI.
#[derive(Parser)]
#[command(name = "pixi-pack", version, about = "A command line tool to pack conda environments for easy sharing", long_about = None)]
struct Cli {
    /// Environment to pack
    #[arg(short, long, default_value = "default")]
    environment: String,

    /// Platform to pack
    #[arg(short, long, default_value = Platform::current().as_str())]
    platform: Platform,

    /// Authentication file for fetching packages
    #[arg(long)]
    auth_file: Option<PathBuf>,

    /// The path to `pixi.toml`, `pyproject.toml`, or the project directory
    #[arg(default_value = cwd().into_os_string())]
    manifest_path: PathBuf,

    /// Output file to write the pack to
    #[arg(short, long)]
    output_file: Option<PathBuf>,

    /// Use a cache directory for downloaded packages
    #[arg(long)]
    use_cache: Option<PathBuf>,

    /// Inject an additional conda package into the final prefix
    #[arg(short, long, num_args(0..))]
    inject: Vec<PathBuf>,

    /// PyPI source distributions are not supported.
    /// This flag allows packing even if PyPI source distributions are present.
    #[arg(long, default_value = "false")]
    ignore_pypi_non_wheel: bool,

    /// Create self-extracting executable
    #[arg(long, default_value = "false")]
    create_executable: bool,

    /// Create pack as a folder instead of a tar
    #[arg(long, default_value = "false", conflicts_with = "create_executable")]
    directory_only: bool,

    /// Optional path or URL to a pixi-unpack executable.
    // Ex. /path/to/pixi-unpack/pixi-unpack.exe
    // Ex. https://example.com/pixi-unpack.exe
    #[arg(long, requires = "create_executable")]
    pixi_unpack_source: Option<UrlOrPath>,

    /// Rattler config for mirror or S3 configuration.
    #[arg(long, short)]
    config: Option<PathBuf>,

    #[command(flatten)]
    verbose: Verbosity,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate shell completion script
    Completion {
        /// The shell to generate the completion script for
        #[arg(short, long, value_enum)]
        shell: Shell,
    },
}

fn define_output_mode(create_executable: bool, directory_only: bool) -> OutputMode {
    if directory_only {
        OutputMode::DirectoryOnly
    } else if create_executable {
        OutputMode::CreateExecutable
    } else {
        OutputMode::Archive
    }
}

fn default_output_file(platform: Platform, mode: OutputMode) -> PathBuf {
    match mode {
        OutputMode::Archive => cwd().join("environment.tar"),
        OutputMode::CreateExecutable => {
            if platform.is_windows() {
                cwd().join("environment.ps1")
            } else {
                cwd().join("environment.sh")
            }
        }
        OutputMode::DirectoryOnly => cwd().join("environment"),
    }
}

/* -------------------------------------------- MAIN ------------------------------------------- */

/// The main entrypoint for the pixi-pack CLI.
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(cli.verbose)
        .with_writer(std::io::stderr)
        .init();

    tracing::debug!("Starting pixi-pack CLI");

    let Cli {
        environment,
        platform,
        auth_file,
        manifest_path,
        output_file,
        inject,
        ignore_pypi_non_wheel,
        create_executable,
        directory_only,
        pixi_unpack_source,
        config,
        use_cache,
        command,
        ..
    } = cli;

    match command {
        Some(Commands::Completion { shell }) => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "pixi-pack", &mut io::stdout());
        }
        None => {
            let output_mode = define_output_mode(create_executable, directory_only);
            let output_file =
                output_file.unwrap_or_else(|| default_output_file(platform, output_mode));

            let config = if let Some(config_path) = config {
                let config = Config::load_from_files(vec![&config_path.clone()])
                    .map_err(|e| anyhow::anyhow!("Failed to parse config file: {}", e))?;
                Some(config)
            } else {
                None
            };

            let options = PackOptions {
                environment,
                platform,
                auth_file,
                output_file,
                manifest_path,
                metadata: PixiPackMetadata {
                    version: DEFAULT_PIXI_PACK_VERSION.to_string(),
                    pixi_pack_version: Some(PIXI_PACK_VERSION.to_string()),
                    platform,
                },
                injected_packages: inject,
                ignore_pypi_non_wheel,
                output_mode,
                pixi_unpack_source,
                cache_dir: use_cache,
                config,
            };
            tracing::debug!("Running pack command with options: {:?}", options);
            pack(options).await?;
        }
    };
    tracing::debug!("Finished running pixi-pack");

    Ok(())
}
