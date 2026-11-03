//! Launching a GUI application from a bundled pack.
//!
//! An application bundle (e.g. a macOS `.app`) contains the `pixi-launcher` executable, a pack and a
//! [`LauncherConfig`]. On first launch the pack is unpacked into a per-user data directory, on
//! every launch the environment is activated and the entry point is started.

use std::{
    collections::HashMap,
    fs::File,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use anyhow::{Context, Result, anyhow};
use rattler_conda_types::Platform;
use rattler_shell::{
    activation::{ActivationVariables, Activator, PathModificationBehavior},
    shell,
};
use serde::{Deserialize, Serialize};

use crate::{UnpackOptions, UnpackProgress, unarchive, unpack};

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
    /// Command to start, followed by any user arguments. The program is either the name of an
    /// executable in the environment, e.g. `napari`, or a path with menuinst placeholders like
    /// `{{ PYTHON }}`, see [`menu_placeholders`].
    pub command: Vec<String>,
    /// Path to the pack, relative to the launcher config.
    pub pack_file: PathBuf,
    /// SHA256 of the pack, used to detect whether the installed environment is up to date.
    pub pack_sha256: String,
}

impl LauncherConfig {
    pub fn from_file(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("could not read launcher config {}", path.display()))?;
        // Editors on Windows like to add a byte order mark.
        serde_json::from_str(contents.trim_start_matches('\u{feff}'))
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

/// Look for a launcher config bundled with the running executable.
///
/// - macOS: the executable is in `Foo.app/Contents/MacOS`, resources in `Foo.app/Contents/Resources`
/// - Windows: resources are next to the executable
/// - Flatpak: the executable is in `/app/bin`, resources in `/app/share/pixi-launcher`
pub fn find_bundled_launcher_config() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    [
        exe_dir.join("../Resources").join(LAUNCHER_CONFIG_FILE_NAME),
        exe_dir.join(LAUNCHER_CONFIG_FILE_NAME),
        exe_dir
            .join("../share/pixi-launcher")
            .join(LAUNCHER_CONFIG_FILE_NAME),
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
    let (program, program_args) = resolve_command(&prefix, &config.command, Platform::current())?;
    let env = activation_env(&prefix)?;

    #[cfg(target_os = "macos")]
    let mut command = macos::bundled_command(&install_dir, &prefix, &program)?;
    #[cfg(not(target_os = "macos"))]
    let mut command = Command::new(&program);
    command.args(program_args).args(args).envs(env);
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
    let progress = Arc::new(UnpackProgress::default());
    let _progress = ui::platform_ui().installing(&config.name, progress.clone());
    // Extracted next to the install instead of into the temporary directory, which can be a small
    // tmpfs (e.g. in a Flatpak) or on another file system, where the environment could not
    // hardlink the packages. Removed with old installs if the launcher is interrupted.
    let work_dir = tempfile::tempdir_in(&app_data_dir)?;
    unarchive(&pack_file, work_dir.path())
        .with_context(|| format!("could not extract {}", pack_file.display()))?;
    unpack(UnpackOptions {
        pack_file: work_dir.path().to_path_buf(),
        output_directory: install_dir.to_path_buf(),
        env_name: ENV_NAME.to_string(),
        shell: None,
        progress: Some(progress),
    })
    .await?;
    drop(work_dir);
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
        if !path.is_dir() || path == current {
            continue;
        }
        // Renaming first fails on Windows while the old version is still running, which keeps
        // its install intact instead of half-deleting it. It is removed on a later launch.
        let removing = path.with_extension("removing");
        if path.extension().is_none_or(|e| e != "removing")
            && let Err(e) = std::fs::rename(&path, &removing)
        {
            tracing::warn!(
                "Could not remove {}, it may be in use: {}",
                path.display(),
                e
            );
            continue;
        }
        tracing::info!("Removing old install at {}", path.display());
        if let Err(e) = std::fs::remove_dir_all(&removing) {
            tracing::warn!("Could not remove {}: {}", removing.display(), e);
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

/// Values for the placeholders menuinst supports in commands, like `{{ PYTHON }}`, for an
/// environment at `prefix`.
/// <https://conda.github.io/menuinst/defining-shortcuts/#placeholders>
///
/// Mirrors the private `rattler_menuinst::render::BaseMenuItemPlaceholders`.
pub(crate) fn menu_placeholders(prefix: &Path, platform: Platform) -> HashMap<String, String> {
    let mut vars = vec![
        ("PREFIX", prefix.to_path_buf()),
        ("BASE_PREFIX", prefix.to_path_buf()),
        ("MENU_DIR", prefix.join("Menu")),
        ("HOME", dirs::home_dir().unwrap_or_default()),
    ];
    if platform.is_windows() {
        vars.extend([
            ("PYTHON", prefix.join("python.exe")),
            ("BASE_PYTHON", prefix.join("python.exe")),
            ("PYTHONW", prefix.join("pythonw.exe")),
            ("BASE_PYTHONW", prefix.join("pythonw.exe")),
            ("BIN_DIR", prefix.join("Library").join("bin")),
            ("SCRIPTS_DIR", prefix.join("Scripts")),
        ]);
    } else {
        vars.extend([
            ("PYTHON", prefix.join("bin/python")),
            ("BASE_PYTHON", prefix.join("bin/python")),
            ("BIN_DIR", prefix.join("bin")),
        ]);
    }
    if platform.is_osx() {
        vars.push(("PYTHONAPP", prefix.join("python.app/Contents/MacOS/python")));
    }
    let icon_ext = if platform.is_windows() {
        "ico"
    } else if platform.is_osx() {
        "icns"
    } else {
        "png"
    };
    let env_name = prefix
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    vars.into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string_lossy().into_owned()))
        .chain([
            ("ICON_EXT".to_string(), icon_ext.to_string()),
            ("ENV_NAME".to_string(), env_name.clone()),
            ("DISTRIBUTION_NAME".to_string(), env_name),
        ])
        .collect()
}

/// Replace placeholders like `{{ PREFIX }}` in `text`.
pub(crate) fn replace_placeholders(text: &str, placeholders: &HashMap<String, String>) -> String {
    placeholders
        .iter()
        .fold(text.to_string(), |text, (key, value)| {
            text.replace(&format!("{{{{ {key} }}}}"), value)
                .replace(&format!("{{{{{key}}}}}"), value)
        })
}

/// The program and arguments of `command` for the environment at `prefix`.
fn resolve_command(
    prefix: &Path,
    command: &[String],
    platform: Platform,
) -> Result<(PathBuf, Vec<String>)> {
    let (program, args) = command
        .split_first()
        .ok_or_else(|| anyhow!("the command to start is empty"))?;
    let placeholders = menu_placeholders(prefix, platform);
    let args = args
        .iter()
        .map(|a| replace_placeholders(a, &placeholders))
        .collect();
    let program = if program.contains("{{") {
        let program = PathBuf::from(replace_placeholders(program, &placeholders));
        if !program.is_file() {
            anyhow::bail!("{} not found", program.display());
        }
        program
    } else {
        find_entry_point(prefix, program)?
    };
    Ok((program, args))
}

fn activation_variables() -> ActivationVariables {
    ActivationVariables {
        conda_prefix: None,
        path: None,
        path_modification_behavior: PathModificationBehavior::Prepend,
        current_env: HashMap::new(),
    }
}

/// Environment variables of the activated prefix.
#[cfg(unix)]
fn activation_env(prefix: &Path) -> Result<HashMap<String, String>> {
    let activator = Activator::from_path(prefix, shell::Bash::default(), Platform::current())?;
    Ok(activator.run_activation(activation_variables(), None)?)
}

/// Environment variables of the activated prefix.
///
/// `Activator::run_activation` would open a console window, as the launcher has none, so the
/// activation script is run here with `CREATE_NO_WINDOW`.
#[cfg(windows)]
fn activation_env(prefix: &Path) -> Result<HashMap<String, String>> {
    use std::os::windows::process::CommandExt;

    let activator = Activator::from_path(prefix, shell::CmdExe, Platform::current())?;
    let mut script = activator
        .activation(activation_variables())?
        .script
        .contents()?;
    let script_dir = tempfile::tempdir()?;
    let script_path = script_dir.path().join("activate.bat");
    let env_path = script_dir.path().join("env.txt");
    // Written to a file, as programs run by activation scripts can print anything to stdout.
    script.push_str(&format!("\r\n@set > \"{}\"\r\n", env_path.display()));
    std::fs::write(&script_path, script)?;

    // `/U` makes cmd write UTF-16 instead of the OEM code page, so non-ASCII paths survive.
    let status = Command::new("cmd.exe")
        .args(["/D", "/U", "/C"])
        .arg(&script_path)
        .stdout(std::process::Stdio::null())
        .creation_flags(windows::CREATE_NO_WINDOW)
        .status()
        .context("could not run activation script")?;
    if !status.success() {
        anyhow::bail!("activation script failed with {}", status);
    }
    let bytes = std::fs::read(&env_path).context("activation script printed no environment")?;
    let utf16: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let env = parse_set_output(&String::from_utf16_lossy(&utf16));
    if !env.contains_key("CONDA_PREFIX") {
        anyhow::bail!("activation script did not set CONDA_PREFIX");
    }
    Ok(env)
}

/// Parse the output of cmd's `set`.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_set_output(output: &str) -> HashMap<String, String> {
    output
        .lines()
        .filter_map(|line| line.split_once('='))
        // cmd keeps per-drive working directories in variables like `=C:`.
        .filter(|(key, _)| !key.is_empty())
        .map(|(key, value)| (key.to_string(), value.trim_end_matches('\r').to_string()))
        .collect()
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

/// Start the application and exit. Console entry points, like most conda entry points on
/// Windows, would otherwise open a console window next to the application.
#[cfg(windows)]
fn exec(mut command: Command) -> Result<()> {
    use std::os::windows::process::CommandExt;

    command
        .creation_flags(windows::CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| anyhow!("could not start {:?}: {}", command.get_program(), e))?;
    std::process::exit(0);
}

#[cfg(windows)]
mod windows {
    /// <https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags>
    pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
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
    use std::sync::Arc;

    use indicatif::HumanBytes;

    use crate::UnpackProgress;

    /// Feedback shown while unpacking, hidden again when dropped.
    pub struct Progress(Option<Box<dyn FnOnce()>>);

    impl Progress {
        fn none() -> Self {
            Self(None)
        }

        fn hide_with(hide: impl FnOnce() + 'static) -> Self {
            Self(Some(Box::new(hide)))
        }
    }

    impl Drop for Progress {
        fn drop(&mut self) {
            if let Some(hide) = self.0.take() {
                hide();
            }
        }
    }

    pub trait LaunchUi {
        /// Show the progress of the first-launch setup.
        fn installing(&self, name: &str, progress: Arc<UnpackProgress>) -> Progress;
        fn error(&self, name: &str, message: &str);
    }

    /// Set to log to stderr instead of showing dialogs, e.g. in CI where a dialog would block.
    const NO_UI_ENV: &str = "PIXI_LAUNCHER_NO_UI";

    /// How often a progress window is updated.
    const UPDATE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

    fn setup_text(name: &str) -> String {
        format!("Setting up {name} for first use. {name} will open automatically.")
    }

    /// The fraction done and a status line, `None` while the total is not known yet.
    fn status(progress: &UnpackProgress) -> Option<(f64, String)> {
        let (done, total) = progress.get();
        if total == 0 {
            None
        } else if done >= total {
            Some((1.0, "Installing...".to_string()))
        } else {
            let status = format!("{} of {}", HumanBytes(done), HumanBytes(total));
            Some((done as f64 / total as f64, status))
        }
    }

    pub fn platform_ui() -> Box<dyn LaunchUi> {
        if std::env::var_os(NO_UI_ENV).is_some() {
            return Box::new(LogUi);
        }
        #[cfg(target_os = "macos")]
        return Box::new(macos::MacUi);
        #[cfg(windows)]
        return Box::new(windows::WindowsUi);
        #[cfg(not(any(target_os = "macos", windows)))]
        return Box::new(zenity::ZenityUi);
    }

    struct LogUi;
    impl LaunchUi for LogUi {
        fn installing(&self, name: &str, _progress: Arc<UnpackProgress>) -> Progress {
            eprintln!("⏳ {}", setup_text(name));
            Progress::none()
        }
        fn error(&self, name: &str, message: &str) {
            eprintln!("❌ {name} could not be started: {message}");
        }
    }

    /// Creates a window with `show` on a new thread and runs `update` on it every
    /// [`UPDATE_INTERVAL`] until the returned [`Progress`] is dropped, then runs `hide`.
    fn poll<W>(
        show: impl FnOnce() -> W + Send + 'static,
        mut update: impl FnMut(&mut W) + Send + 'static,
        hide: impl FnOnce(W) + Send + 'static,
    ) -> Progress {
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            let mut window = show();
            while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                stopped.recv_timeout(UPDATE_INTERVAL)
            {
                update(&mut window);
            }
            hide(window);
        });
        Progress::hide_with(move || {
            drop(stop);
            let _ = thread.join();
        })
    }

    #[cfg(windows)]
    mod windows {
        use std::sync::Arc;

        use windows::{
            Win32::{
                System::Com::{
                    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance,
                    CoInitializeEx,
                },
                UI::Shell::{
                    CLSID_ProgressDialog, IProgressDialog, PROGDLG_NOCANCEL, PROGDLG_NOMINIMIZE,
                },
            },
            core::HSTRING,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};

        use super::{LaunchUi, Progress, poll, setup_text, status};
        use crate::UnpackProgress;

        pub struct WindowsUi;

        fn wide(s: &str) -> Vec<u16> {
            s.encode_utf16().chain(std::iter::once(0)).collect()
        }

        /// Show the shell's progress dialog, which runs its own window thread.
        fn progress_dialog(title: &str, text: &str) -> windows::core::Result<IProgressDialog> {
            // SAFETY: COM is initialized for the calling thread before creating the dialog, which
            // is only used from that thread. No reserved arguments are passed.
            unsafe {
                CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
                let dialog: IProgressDialog =
                    CoCreateInstance(&CLSID_ProgressDialog, None, CLSCTX_INPROC_SERVER)?;
                dialog.SetTitle(&HSTRING::from(title))?;
                dialog.SetLine(1, &HSTRING::from(text), false, None)?;
                dialog.StartProgressDialog(
                    None,
                    None,
                    PROGDLG_NOCANCEL | PROGDLG_NOMINIMIZE,
                    None,
                )?;
                Ok(dialog)
            }
        }

        impl LaunchUi for WindowsUi {
            fn installing(&self, name: &str, progress: Arc<UnpackProgress>) -> Progress {
                let (title, text) = (format!("{name} setup"), setup_text(name));
                // The dialog is created on the polling thread, as COM objects belong to the
                // thread that created them.
                poll(
                    move || {
                        progress_dialog(&title, &text)
                            .inspect_err(|e| tracing::warn!("could not show progress: {e}"))
                            .ok()
                    },
                    move |dialog| {
                        if let (Some(dialog), Some((fraction, line))) =
                            (dialog.as_ref(), status(&progress))
                        {
                            // SAFETY: the dialog was created on this thread.
                            unsafe {
                                let _ = dialog.SetProgress64((fraction * 1000.0) as u64, 1000);
                                let _ = dialog.SetLine(2, &HSTRING::from(line), false, None);
                            }
                        }
                    },
                    |dialog| {
                        if let Some(dialog) = dialog {
                            // SAFETY: the dialog was created on this thread.
                            let _ = unsafe { dialog.StopProgressDialog() };
                        }
                    },
                )
            }

            fn error(&self, name: &str, message: &str) {
                let (title, text) = (wide(&format!("{name} could not be started")), wide(message));
                // SAFETY: both strings are NUL-terminated UTF-16 and outlive the call.
                unsafe {
                    MessageBoxW(
                        std::ptr::null_mut(),
                        text.as_ptr(),
                        title.as_ptr(),
                        MB_OK | MB_ICONERROR,
                    );
                }
            }
        }
    }

    /// Dialogs with `zenity`, which e.g. the Freedesktop and GNOME Flatpak runtimes include.
    #[cfg(not(any(target_os = "macos", windows)))]
    mod zenity {
        use std::{
            io::Write,
            process::{Command, Stdio},
            sync::Arc,
        };

        use super::{LaunchUi, LogUi, Progress, poll, setup_text, status};
        use crate::UnpackProgress;

        pub struct ZenityUi;

        impl LaunchUi for ZenityUi {
            fn installing(&self, name: &str, progress: Arc<UnpackProgress>) -> Progress {
                // Reads the percentage and `# text` lines from stdin.
                let child = Command::new("zenity")
                    .args(["--progress", "--no-cancel", "--width=420"])
                    .arg(format!("--title={name}"))
                    .arg(format!("--text={}", setup_text(name)))
                    .stdin(Stdio::piped())
                    .spawn();
                let mut child = match child {
                    Ok(child) => child,
                    Err(e) => {
                        tracing::warn!("could not show progress: {e}");
                        return LogUi.installing(name, progress);
                    }
                };
                let text = setup_text(name);
                // Pulses until the total is known.
                if let Some(stdin) = child.stdin.as_mut() {
                    let _ = writeln!(stdin, "pulsate:true");
                }
                poll(
                    move || child,
                    move |child| {
                        if let (Some(stdin), Some((fraction, line))) =
                            (child.stdin.as_mut(), status(&progress))
                        {
                            // 100 would close the dialog, it is closed when done.
                            let percent = (fraction * 100.0).min(99.0) as u32;
                            let _ =
                                writeln!(stdin, "pulsate:false\n{percent}\n# {text}\\n\\n{line}");
                        }
                    },
                    |mut child| {
                        let _ = child.kill();
                        let _ = child.wait();
                    },
                )
            }

            fn error(&self, name: &str, message: &str) {
                let status = Command::new("zenity")
                    .args(["--error", "--no-markup", "--width=420"])
                    .arg(format!("--title={name} could not be started"))
                    .arg(format!("--text={message}"))
                    .status();
                if !status.is_ok_and(|s| s.success()) {
                    LogUi.error(name, message);
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    mod macos {
        use std::{process::Command, sync::Arc};

        use super::{LaunchUi, Progress, poll, setup_text, status};
        use crate::UnpackProgress;

        pub struct MacUi;

        /// A window with a progress bar, run with `osascript` to avoid linking AppKit. It reads the
        /// fraction done and a status line from the file passed as third argument.
        const PROGRESS_WINDOW: &str = include_str!("launch/progress-window.js");

        /// Quote a string for use in AppleScript.
        fn quote(s: &str) -> String {
            format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
        }

        impl LaunchUi for MacUi {
            fn installing(&self, name: &str, progress: Arc<UnpackProgress>) -> Progress {
                let status_file = match tempfile::NamedTempFile::new() {
                    Ok(file) => file,
                    Err(e) => {
                        tracing::warn!("could not show progress: {e}");
                        return Progress::none();
                    }
                };
                let child = Command::new("osascript")
                    .args([
                        "-l",
                        "JavaScript",
                        "-e",
                        PROGRESS_WINDOW,
                        name,
                        &setup_text(name),
                    ])
                    .arg(status_file.path())
                    .spawn();
                let child = match child {
                    Ok(child) => child,
                    Err(e) => {
                        tracing::warn!("could not show progress: {e}");
                        return Progress::none();
                    }
                };
                poll(
                    move || child,
                    move |_| {
                        if let Some((fraction, line)) = status(&progress) {
                            // Written whole and renamed, so the window never reads a partial file.
                            let path = status_file.path().with_extension("new");
                            if std::fs::write(&path, format!("{fraction}\n{line}")).is_ok() {
                                let _ = std::fs::rename(&path, status_file.path());
                            }
                        }
                    },
                    |mut child| {
                        let _ = child.kill();
                        let _ = child.wait();
                    },
                )
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
            command: vec!["napari".to_string()],
            pack_file: PathBuf::from("pack.tar"),
            pack_sha256: "0123456789abcdef0123456789abcdef".to_string(),
        }
    }

    #[test]
    fn test_launcher_config_roundtrip() {
        let config = config();
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"command\":[\"napari\"]"));
        assert_eq!(
            serde_json::from_str::<LauncherConfig>(&json).unwrap(),
            config
        );
    }

    #[test]
    fn test_parse_set_output() {
        let output =
            "=C:=C:\\Users\r\nCONDA_PREFIX=C:\\env\r\nPATH=C:\\env;C:\\Windows\r\nEQ=a=b\r\n";
        let env = parse_set_output(output);
        assert_eq!(env.len(), 3);
        assert_eq!(env["CONDA_PREFIX"], "C:\\env");
        assert_eq!(env["PATH"], "C:\\env;C:\\Windows");
        assert_eq!(env["EQ"], "a=b");
    }

    #[test]
    fn test_replace_placeholders() {
        let prefix = Path::new("/opt/env");
        let placeholders = menu_placeholders(prefix, Platform::Linux64);
        // Paths are joined with the separator of the host.
        assert_eq!(
            replace_placeholders(
                "{{ PYTHON }} -m x {{MENU_DIR}}/x.{{ ICON_EXT }}",
                &placeholders
            ),
            format!(
                "{} -m x {}/x.png",
                prefix.join("bin/python").display(),
                prefix.join("Menu").display()
            )
        );
        let placeholders = menu_placeholders(Path::new("C:\\env"), Platform::Win64);
        assert!(placeholders["PYTHONW"].ends_with("pythonw.exe"));
        assert_eq!(placeholders["ICON_EXT"], "ico");
    }

    #[test]
    fn test_resolve_command() {
        let prefix = tempfile::tempdir().unwrap();
        let platform = Platform::current();
        let python = menu_placeholders(prefix.path(), platform)["PYTHON"].clone();
        let command = ["{{ PYTHON }}", "-m", "napari", "{{ PREFIX }}"].map(String::from);
        assert!(resolve_command(prefix.path(), &command, platform).is_err());

        std::fs::create_dir_all(Path::new(&python).parent().unwrap()).unwrap();
        std::fs::write(&python, "").unwrap();
        let (program, args) = resolve_command(prefix.path(), &command, platform).unwrap();
        assert_eq!(program, PathBuf::from(&python));
        assert_eq!(
            args,
            vec![
                "-m".to_string(),
                "napari".to_string(),
                prefix.path().to_string_lossy().into_owned()
            ]
        );
        assert!(resolve_command(prefix.path(), &[], platform).is_err());
    }

    #[test]
    fn test_launcher_config_with_byte_order_mark() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LAUNCHER_CONFIG_FILE_NAME);
        let json = serde_json::to_string(&config()).unwrap();
        std::fs::write(&path, format!("\u{feff}{json}")).unwrap();
        assert_eq!(LauncherConfig::from_file(&path).unwrap(), config());
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
