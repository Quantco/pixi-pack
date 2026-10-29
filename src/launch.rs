//! Launching a GUI application from a bundled pack.
//!
//! An application bundle (e.g. a macOS `.app`) contains the `pixi-unpack` executable, a pack and a
//! [`LauncherConfig`]. On first launch the pack is unpacked into a per-user data directory, on
//! every launch the environment is activated and the entry point is started.

use std::{
    collections::HashMap,
    fs::File,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, anyhow};
use rattler_conda_types::Platform;
use rattler_shell::{
    activation::{ActivationVariables, Activator, PathModificationBehavior},
    shell,
};
use serde::{Deserialize, Serialize};

use crate::{UnpackOptions, unpack};

/// File name of the launcher config inside an application bundle.
pub const LAUNCHER_CONFIG_FILE_NAME: &str = "pixi-launcher.json";

const ENV_NAME: &str = "env";
const COMPLETE_MARKER: &str = ".pixi-unpack-complete";

/// Describes the application that is launched from a bundled pack.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct LauncherConfig {
    /// Human readable name of the application.
    pub name: String,
    /// Reverse-DNS identifier, used as the name of the data directory.
    pub identifier: String,
    /// Version of the application.
    pub version: String,
    /// Name of the executable in the environment to start, e.g. `napari`.
    pub entry_point: String,
    /// Arguments always passed to the entry point, before any user arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// Path to the pack, relative to the launcher config.
    pub pack_file: PathBuf,
    /// SHA256 of the pack, used to detect whether the installed environment is up to date.
    pub pack_sha256: String,
}

impl LauncherConfig {
    pub fn from_file(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("could not read launcher config {}", path.display()))?;
        serde_json::from_str(&contents)
            .with_context(|| format!("could not parse launcher config {}", path.display()))
    }

    /// Directory holding all installed versions of this application.
    pub fn app_data_dir(&self) -> Result<PathBuf> {
        let base = dirs::data_local_dir()
            .ok_or_else(|| anyhow!("could not determine the user data directory"))?;
        Ok(base.join(&self.identifier))
    }

    /// Directory the current version of the pack is unpacked to.
    pub fn install_dir(&self) -> Result<PathBuf> {
        let stamp: String = self.pack_sha256.chars().take(16).collect();
        Ok(self.app_data_dir()?.join(stamp))
    }
}

/// Look for a launcher config bundled next to the running executable.
///
/// On macOS the executable lives in `Foo.app/Contents/MacOS` and resources in
/// `Foo.app/Contents/Resources`, elsewhere the config is expected next to the executable.
pub fn find_bundled_launcher_config() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    [
        exe_dir.join("../Resources").join(LAUNCHER_CONFIG_FILE_NAME),
        exe_dir.join(LAUNCHER_CONFIG_FILE_NAME),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// Unpack the pack if needed, then start the entry point.
///
/// On Unix this replaces the current process and only returns on error.
pub async fn launch(config_path: &Path, args: Vec<String>) -> Result<()> {
    let config = LauncherConfig::from_file(config_path)?;
    let ui = ui::platform_ui();
    match launch_impl(config_path, &config, args).await {
        Ok(()) => Ok(()),
        Err(e) => {
            ui.error(&config.name, &format!("{e:#}"));
            Err(e)
        }
    }
}

async fn launch_impl(config_path: &Path, config: &LauncherConfig, args: Vec<String>) -> Result<()> {
    let install_dir = config.install_dir()?;
    ensure_installed(config_path, config, &install_dir).await?;

    let prefix = install_dir.join(ENV_NAME);
    let entry_point = find_entry_point(&prefix, &config.entry_point)?;
    let env = activation_env(&prefix)?;

    #[cfg(target_os = "macos")]
    let mut command = macos::bundled_command(&install_dir, &prefix, &entry_point)?;
    #[cfg(not(target_os = "macos"))]
    let mut command = Command::new(&entry_point);
    command.args(&config.args).args(args).envs(env);
    tracing::info!(
        "Starting {:?} {:?}",
        command.get_program(),
        command.get_args()
    );
    exec(command)
}

/// Unpack the pack into `install_dir` unless that was already done.
///
/// The pack is unpacked in place, because conda environments are not relocatable.
/// A marker file written at the very end tells complete installs from interrupted ones.
async fn ensure_installed(
    config_path: &Path,
    config: &LauncherConfig,
    install_dir: &Path,
) -> Result<()> {
    let marker = install_dir.join(COMPLETE_MARKER);
    if marker.exists() {
        return Ok(());
    }

    let app_data_dir = config.app_data_dir()?;
    std::fs::create_dir_all(&app_data_dir)
        .with_context(|| format!("could not create {}", app_data_dir.display()))?;

    // Prevent two instances started at the same time from unpacking concurrently.
    let lock = File::create(app_data_dir.join(".lock"))?;
    lock.lock()?;
    if marker.exists() {
        return Ok(());
    }

    if install_dir.exists() {
        tracing::info!("Removing incomplete install at {}", install_dir.display());
        std::fs::remove_dir_all(install_dir)?;
    }

    let pack_file = config_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(&config.pack_file);
    let _progress = ui::platform_ui().installing(&config.name);
    unpack(UnpackOptions {
        pack_file,
        output_directory: install_dir.to_path_buf(),
        env_name: ENV_NAME.to_string(),
        shell: None,
    })
    .await?;
    std::fs::write(&marker, &config.version)?;

    remove_old_installs(&app_data_dir, install_dir);
    Ok(())
}

/// Remove installs of other versions of the application. Failures are only logged.
fn remove_old_installs(app_data_dir: &Path, current: &Path) {
    let Ok(entries) = std::fs::read_dir(app_data_dir) else {
        return;
    };
    for path in entries.flatten().map(|e| e.path()) {
        if path.is_dir() && path != current {
            tracing::info!("Removing old install at {}", path.display());
            if let Err(e) = std::fs::remove_dir_all(&path) {
                tracing::warn!("Could not remove {}: {}", path.display(), e);
            }
        }
    }
}

fn find_entry_point(prefix: &Path, name: &str) -> Result<PathBuf> {
    let candidates = if Platform::current().is_windows() {
        let exe = format!("{name}.exe");
        vec![
            prefix.join("Scripts").join(&exe),
            prefix.join(&exe),
            prefix.join("Library").join("bin").join(&exe),
        ]
    } else {
        vec![prefix.join("bin").join(name)]
    };
    candidates
        .iter()
        .find(|p| p.is_file())
        .cloned()
        .ok_or_else(|| anyhow!("entry point `{}` not found in {}", name, prefix.display()))
}

/// Environment variables of the activated prefix.
fn activation_env(prefix: &Path) -> Result<HashMap<String, String>> {
    let variables = ActivationVariables {
        conda_prefix: None,
        path: None,
        path_modification_behavior: PathModificationBehavior::Prepend,
        current_env: HashMap::new(),
    };
    let env = if Platform::current().is_windows() {
        Activator::from_path(prefix, shell::CmdExe, Platform::current())?
            .run_activation(variables, None)?
    } else {
        Activator::from_path(prefix, shell::Bash::default(), Platform::current())?
            .run_activation(variables, None)?
    };
    Ok(env)
}

#[cfg(unix)]
fn exec(mut command: Command) -> Result<()> {
    use std::os::unix::process::CommandExt;
    // Only returns on failure.
    let err = command.exec();
    Err(anyhow!(
        "could not start {:?}: {}",
        command.get_program(),
        err
    ))
}

#[cfg(not(unix))]
fn exec(mut command: Command) -> Result<()> {
    let status = command
        .status()
        .map_err(|e| anyhow!("could not start {:?}: {}", command.get_program(), e))?;
    std::process::exit(status.code().unwrap_or(1));
}

/// macOS shows a running program in the Dock as the application bundle its executable lives in.
/// Executing the entry point directly would show up as e.g. `python3.13` with a generic icon, so
/// it is started from a copy of the application bundle next to the environment instead, whose
/// executable is a hard link to the actual program.
#[cfg(target_os = "macos")]
mod macos {
    use std::{
        io::{BufRead, BufReader, Read},
        os::unix::fs::MetadataExt,
        path::{Path, PathBuf},
        process::Command,
    };

    use anyhow::{Context, Result};

    const MACH_O_MAGIC_NUMBERS: [u32; 5] =
        [0xfeedface, 0xfeedfacf, 0xcafebabe, 0xcefaedfe, 0xcffaedfe];

    /// The command to start `entry_point` from a bundle in `install_dir`.
    /// Falls back to the plain entry point when not running from a bundle.
    pub fn bundled_command(
        install_dir: &Path,
        prefix: &Path,
        entry_point: &Path,
    ) -> Result<Command> {
        let Some((bundle, executable_name)) = running_bundle() else {
            return Ok(Command::new(entry_point));
        };
        let (program, script) = if is_mach_o(entry_point)? {
            // Entry points like `python` are symlinks, which would be hard linked as such.
            (entry_point.canonicalize()?, None)
        } else if is_python_script(entry_point)? {
            let python = prefix.join("bin").join("python").canonicalize()?;
            (python, Some(entry_point))
        } else {
            tracing::warn!(
                "{} is neither a binary nor a python script, starting it directly",
                entry_point.display()
            );
            return Ok(Command::new(entry_point));
        };

        let bundle_name = bundle.file_name().context("bundle has no name")?;
        let stub = install_dir.join(bundle_name);
        let stub_executable = stub.join("Contents/MacOS").join(&executable_name);
        if !is_same_file(&stub_executable, &program) {
            create_stub_bundle(&bundle, &stub, &stub_executable, &program)?;
        }

        let mut command = Command::new(stub_executable);
        command.args(script);
        Ok(command)
    }

    /// The application bundle containing the running executable, and the executable's name.
    fn running_bundle() -> Option<(PathBuf, String)> {
        let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
        let contents = exe.parent()?.parent()?;
        if !contents.join("Info.plist").is_file() {
            return None;
        }
        let bundle = contents.parent()?;
        if bundle.extension()? != "app" {
            return None;
        }
        Some((bundle.to_path_buf(), exe.file_name()?.to_str()?.to_string()))
    }

    /// Create a bundle with the same `Info.plist` and icons as `bundle`, but `program` as its
    /// executable.
    fn create_stub_bundle(
        bundle: &Path,
        stub: &Path,
        stub_executable: &Path,
        program: &Path,
    ) -> Result<()> {
        tracing::info!("Creating application bundle at {}", stub.display());
        if stub.exists() {
            std::fs::remove_dir_all(stub)?;
        }
        let resources = stub.join("Contents/Resources");
        std::fs::create_dir_all(&resources)?;
        std::fs::create_dir_all(stub.join("Contents/MacOS"))?;
        std::fs::copy(
            bundle.join("Contents/Info.plist"),
            stub.join("Contents/Info.plist"),
        )?;
        for entry in std::fs::read_dir(bundle.join("Contents/Resources"))?.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "icns") {
                std::fs::copy(&path, resources.join(entry.file_name()))?;
            }
        }
        // Conda environments are not relocatable, so the program must stay where it is. A hard
        // link gives it a second path inside the bundle without moving it.
        if std::fs::hard_link(program, stub_executable).is_err() {
            std::fs::copy(program, stub_executable)?;
        }
        Ok(())
    }

    fn is_same_file(a: &Path, b: &Path) -> bool {
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        }
    }

    fn is_mach_o(path: &Path) -> Result<bool> {
        let mut magic = [0u8; 4];
        let mut file = std::fs::File::open(path)?;
        if file.read_exact(&mut magic).is_err() {
            return Ok(false);
        }
        Ok(MACH_O_MAGIC_NUMBERS.contains(&u32::from_be_bytes(magic)))
    }

    /// Entry points of python packages either have a python shebang or, for long prefixes or
    /// prefixes with spaces, a `/bin/sh` shebang followed by a line re-executing python.
    fn is_python_script(path: &Path) -> Result<bool> {
        let file = std::fs::File::open(path)?;
        let mut lines = BufReader::new(file).lines().map_while(Result::ok);
        let first = lines.next().unwrap_or_default();
        let second = lines.next().unwrap_or_default();
        Ok(first.starts_with("#!") && (first.contains("python") || second.starts_with("'''exec'")))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_is_python_script() {
            let dir = tempfile::tempdir().unwrap();
            let check = |contents: &str| {
                let path = dir.path().join("script");
                std::fs::write(&path, contents).unwrap();
                is_python_script(&path).unwrap()
            };
            assert!(check("#!/opt/env/bin/python3.13\nimport sys\n"));
            assert!(check(
                "#!/bin/sh\n'''exec' \"/a b/env/bin/python3.13\" \"$0\" \"$@\" #'''\n"
            ));
            assert!(!check("#!/bin/sh\necho hi\n"));
            assert!(!check(""));
        }

        #[test]
        fn test_is_mach_o() {
            assert!(is_mach_o(&std::env::current_exe().unwrap()).unwrap());
            let dir = tempfile::tempdir().unwrap();
            let script = dir.path().join("script");
            std::fs::write(&script, "#!/bin/sh\n").unwrap();
            assert!(!is_mach_o(&script).unwrap());
        }
    }
}

/// Minimal native feedback, as a launched GUI application has no terminal.
mod ui {
    use std::process::Child;

    /// Feedback shown while unpacking, hidden again when dropped.
    pub struct Progress(Option<Child>);

    impl Drop for Progress {
        fn drop(&mut self) {
            if let Some(child) = &mut self.0 {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    pub trait LaunchUi {
        /// Show that the first-launch setup is running.
        fn installing(&self, name: &str) -> Progress;
        fn error(&self, name: &str, message: &str);
    }

    pub fn platform_ui() -> Box<dyn LaunchUi> {
        #[cfg(target_os = "macos")]
        return Box::new(macos::MacUi);
        #[cfg(not(target_os = "macos"))]
        return Box::new(LogUi);
    }

    #[cfg_attr(target_os = "macos", allow(dead_code))]
    struct LogUi;
    impl LaunchUi for LogUi {
        fn installing(&self, name: &str) -> Progress {
            eprintln!("⏳ Setting up {name} for first use...");
            Progress(None)
        }
        fn error(&self, name: &str, message: &str) {
            eprintln!("❌ {name} could not be started: {message}");
        }
    }

    #[cfg(target_os = "macos")]
    mod macos {
        use std::process::Command;

        use super::{LaunchUi, Progress};

        pub struct MacUi;

        /// Quote a string for use in AppleScript.
        fn quote(s: &str) -> String {
            format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
        }

        impl LaunchUi for MacUi {
            fn installing(&self, name: &str) -> Progress {
                let script = format!(
                    "display dialog {} with title {} buttons {{\"OK\"}} default button 1 with icon note",
                    quote(&format!(
                        "Setting up {name} for first use.\n\nThis can take a minute, {name} will open automatically."
                    )),
                    quote(name),
                );
                match Command::new("osascript").args(["-e", &script]).spawn() {
                    Ok(child) => Progress(Some(child)),
                    Err(e) => {
                        tracing::warn!("could not show progress dialog: {e}");
                        Progress(None)
                    }
                }
            }

            fn error(&self, name: &str, message: &str) {
                let script = format!(
                    "display alert {} message {} as critical",
                    quote(&format!("{name} could not be started")),
                    quote(message),
                );
                let _ = Command::new("osascript").args(["-e", &script]).status();
            }
        }
    }
}

/* --------------------------------------------------------------------------------------------- */
/*                                             TESTS                                             */
/* --------------------------------------------------------------------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> LauncherConfig {
        LauncherConfig {
            name: "Ocean Viewer".to_string(),
            identifier: "org.example.ocean-viewer".to_string(),
            version: "1.0.0".to_string(),
            entry_point: "napari".to_string(),
            args: vec![],
            pack_file: PathBuf::from("pack.tar"),
            pack_sha256: "0123456789abcdef0123456789abcdef".to_string(),
        }
    }

    #[test]
    fn test_launcher_config_roundtrip() {
        let config = config();
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"entry-point\":\"napari\""));
        assert_eq!(
            serde_json::from_str::<LauncherConfig>(&json).unwrap(),
            config
        );
    }

    #[test]
    fn test_install_dir_uses_pack_hash() {
        let install_dir = config().install_dir().unwrap();
        assert!(install_dir.ends_with("org.example.ocean-viewer/0123456789abcdef"));
    }

    #[test]
    fn test_find_entry_point() {
        let prefix = tempfile::tempdir().unwrap();
        assert!(find_entry_point(prefix.path(), "napari").is_err());
        let bin = if Platform::current().is_windows() {
            prefix.path().join("Scripts").join("napari.exe")
        } else {
            prefix.path().join("bin").join("napari")
        };
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, "").unwrap();
        assert_eq!(find_entry_point(prefix.path(), "napari").unwrap(), bin);
    }
}
