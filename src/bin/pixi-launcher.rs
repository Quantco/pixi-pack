// A GUI program on Windows, so starting it does not open a console window.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use pixi_pack::{LauncherConfig, find_bundled_launcher_config, launch};

/* -------------------------------------------- MAIN ------------------------------------------- */

/// The main entrypoint of application bundles created with `pixi-pack --create-app`.
///
/// Uses the launcher config bundled next to the executable, or the one passed with
/// `--config <path>` as the first arguments. All other arguments are passed on to the
/// application.
#[tokio::main]
async fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let config_path = if args.first().map(String::as_str) == Some("--config") {
        let path = args
            .get(1)
            .ok_or_else(|| anyhow!("--config requires a path"))?;
        let path = PathBuf::from(path);
        args.drain(..2);
        path
    } else {
        find_bundled_launcher_config()
            .ok_or_else(|| anyhow!("no launcher config found next to the executable"))?
    };

    init_logging(&config_path);
    launch(&config_path, args).await
}

/// A launched application has no terminal, so log to a file in its data directory instead.
fn init_logging(config_path: &std::path::Path) {
    let log_file = LauncherConfig::from_file(config_path)
        .and_then(|config| config.app_data_dir())
        .ok()
        .and_then(|dir| {
            std::fs::create_dir_all(&dir).ok()?;
            std::fs::File::create(dir.join("launcher.log")).ok()
        });
    let builder = tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(tracing::Level::INFO)
        .with_ansi(false);
    match log_file {
        Some(file) => builder.with_writer(std::sync::Mutex::new(file)).init(),
        None => builder.with_writer(std::io::stderr).init(),
    }
}
