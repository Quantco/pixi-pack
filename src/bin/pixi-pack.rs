use std::io;
use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use clap_verbosity_flag::Verbosity;
use rattler_conda_types::Platform;

use anyhow::Result;
use pixi_pack::{Config, OutputMode, PackOptions, PixiPackMetadata, pack};
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

    /// Platform to pack: a conda subdir (e.g. `linux-64`) or a platform name
    /// from the lockfile (e.g. `jetson`)
    #[arg(short, long, default_value = Platform::current().as_str())]
    platform: String,

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

    /// Create an application bundle (`.app`/`.dmg`) described by the given TOML file.
    /// `--output-file` is the directory the bundles are written to.
    #[cfg(feature = "app")]
    #[arg(long, conflicts_with_all = ["create_executable", "directory_only"])]
    create_app: Option<PathBuf>,

    /// Optional path or URL to a pixi-unpack executable.
    // Ex. /path/to/pixi-unpack/pixi-unpack.exe
    // Ex. https://example.com/pixi-unpack.exe
    #[arg(long)]
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
        #[cfg(feature = "app")]
        create_app,
        ..
    } = cli;

    if pixi_unpack_source.is_some() && !create_executable {
        #[cfg(feature = "app")]
        let has_app = create_app.is_some();
        #[cfg(not(feature = "app"))]
        let has_app = false;
        if !has_app {
            anyhow::bail!("--pixi-unpack-source requires --create-executable or --create-app");
        }
    }

    match command {
        Some(Commands::Completion { shell }) => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "pixi-pack", &mut io::stdout());
        }
        None => {
            let output_mode = define_output_mode(create_executable, directory_only);
            #[cfg(feature = "app")]
            let app_out_dir = output_file.clone().unwrap_or_else(cwd);

            let output_file = match output_file {
                Some(file) => file,
                None => match output_mode {
                    OutputMode::Archive => cwd().join("environment.tar"),
                    OutputMode::DirectoryOnly => cwd().join("environment"),
                    OutputMode::CreateExecutable => {
                        // A named platform can point at any subdir, so ask the
                        // lockfile before picking `.ps1` or `.sh`.
                        let subdir =
                            pixi_pack::resolve_platform(&manifest_path, &environment, &platform)?;
                        if subdir.is_windows() {
                            cwd().join("environment.ps1")
                        } else {
                            cwd().join("environment.sh")
                        }
                    }
                },
            };

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
                metadata: PixiPackMetadata::default(),
                injected_packages: inject,
                ignore_pypi_non_wheel,
                output_mode,
                pixi_unpack_source,
                cache_dir: use_cache,
                config,
            };
            #[cfg(feature = "app")]
            if let Some(app_config) = create_app {
                let app_config = pixi_pack::AppConfig::from_file(&app_config)?;
                tracing::debug!("Running create-app with options: {:?}", options);
                pixi_pack::create_app(options, app_config, &app_out_dir).await?;
                return Ok(());
            }
            tracing::debug!("Running pack command with options: {:?}", options);
            pack(options).await?;
        }
    };
    tracing::debug!("Finished running pixi-pack");

    Ok(())
}
