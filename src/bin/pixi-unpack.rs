use std::io;
use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use clap_verbosity_flag::Verbosity;

use anyhow::Result;
use pixi_pack::{UnpackOptions, unpack};
use rattler_shell::shell::ShellEnum;

/* -------------------------------------------- CLI -------------------------------------------- */

fn cwd() -> PathBuf {
    std::env::current_dir().expect("failed to obtain current working directory")
}

/// The pixi-unpack CLI.
#[derive(Parser)]
#[command(version, about, long_about = None)]
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
    #[arg()]
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
}

/* -------------------------------------------- MAIN ------------------------------------------- */

/// The main entrypoint for the pixi-pack CLI.
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(cli.verbose)
        .init();

    tracing::debug!("Starting pixi-pack CLI");

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
            generate(shell, &mut cmd, "pixi-pack", &mut io::stdout());
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
    tracing::debug!("Finished running pixi-pack");

    Ok(())
}
