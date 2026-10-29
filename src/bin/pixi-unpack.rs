use std::io;
use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use clap_verbosity_flag::Verbosity;

use anyhow::Result;
use pixi_pack::{UnpackOptions, find_bundled_launcher_config, launch, unpack};
use rattler_shell::shell::ShellEnum;

/* -------------------------------------------- CLI -------------------------------------------- */

fn cwd() -> PathBuf {
    std::env::current_dir().expect("failed to obtain current working directory")
}

/// The pixi-unpack CLI.
#[derive(Parser)]
#[command(name = "pixi-unpack", version, about = "A command line tool to unpack environments created by pixi-pack", long_about = None)]
struct Cli {
    /// Where to unpack the environment.
    /// The environment will be unpacked into a subdirectory of this path
    /// (default `env`, change with `--env-name`).
    /// The activation script will be written to the root of this path.
    #[arg(short, long, default_value = cwd().into_os_string())]
    output_directory: PathBuf,

    /// Name of the environment
    #[arg(short, long, default_value = "env")]
    env_name: String,

    /// Path to the pack file
    #[arg(default_value = cwd().join("environment.tar").into_os_string())]
    pack_file: PathBuf,

    /// Sets the shell, options: [`bash`, `zsh`, `xonsh`, `cmd`, `powershell`, `fish`, `nushell`]
    #[arg(short, long)]
    shell: Option<ShellEnum>,

    #[command(subcommand)]
    command: Option<Commands>,

    #[command(flatten)]
    verbose: Verbosity,
}

/// The subcommands for the pixi-unpack CLI.
#[derive(Subcommand)]
enum Commands {
    /// Generate shell completion script
    Completion {
        /// The shell to generate the completion script for
        #[arg(short, long, value_enum)]
        shell: Shell,
    },
    /// Unpack the environment on first use and start an application from it.
    /// This is what application bundles created with `pixi-pack --create-app` run.
    Launch {
        /// Path to the launcher config (`pixi-launcher.json`)
        #[arg(long)]
        config: PathBuf,
        /// Arguments passed on to the application
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

/// A launched application has no terminal, so log to a file instead.
fn init_launch_logging() {
    let log_file = dirs::cache_dir()
        .map(|dir| dir.join("pixi-unpack-launch.log"))
        .and_then(|path| std::fs::File::create(path).ok());
    let builder = tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(tracing::Level::INFO)
        .with_ansi(false);
    match log_file {
        Some(file) => builder.with_writer(std::sync::Mutex::new(file)).init(),
        None => builder.with_writer(std::io::stderr).init(),
    }
}

/* -------------------------------------------- MAIN ------------------------------------------- */

/// The main entrypoint for the pixi-unpack CLI.
#[tokio::main]
async fn main() -> Result<()> {
    // Inside an application bundle, all arguments belong to the application.
    if let Some(config) = find_bundled_launcher_config() {
        init_launch_logging();
        let args = std::env::args().skip(1).collect();
        return launch(&config, args).await;
    }

    let cli = Cli::parse();

    tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(cli.verbose)
        .with_writer(std::io::stderr)
        .init();

    tracing::debug!("Starting pixi-unpack CLI");

    let Cli {
        output_directory,
        env_name,
        pack_file,
        shell,
        command,
        ..
    } = cli;

    match command {
        Some(Commands::Completion { shell }) => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "pixi-unpack", &mut io::stdout());
        }
        Some(Commands::Launch { config, args }) => {
            launch(&config, args).await?;
        }
        None => {
            let options = UnpackOptions {
                pack_file,
                output_directory,
                env_name,
                shell,
            };
            tracing::debug!("Running unpack command with options: {:?}", options);
            unpack(options).await?;
        }
    };
    tracing::debug!("Finished running pixi-unpack");

    Ok(())
}
