//! Creating a Flatpak bundle (`.flatpak`) with the `flatpak build-*` commands.
//!
//! Flatpak applications always live at `/app` inside the sandbox, so unlike the other formats the
//! environment is unpacked at build time, by running `pixi-unpack` inside the build sandbox. The
//! application then starts the command directly from `/app/env`. If packages are downloaded when
//! unpacking, the Flatpak instead contains `pixi-launcher`, which unpacks the environment into the
//! application's data directory on first start, like on the other platforms.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use rattler_conda_types::Platform;
use serde::Deserialize;

use super::App;
use super::PACK_FILE_NAME;
use crate::{
    LAUNCHER_CONFIG_FILE_NAME, REMOTE_PACKAGES_PATH, menu_placeholders, replace_placeholders,
};

const FLATHUB_REPO: &str = "https://dl.flathub.org/repo/flathub.flatpakrepo";
const PREFIX: &str = "/app/env";
const MAX_ICON_SIZE: u32 = 512;
/// Installing a newer bundle with the same branch replaces the older version.
const BRANCH: &str = "stable";

/// Settings for Flatpak bundles.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct AppFlatpakConfig {
    /// The runtime the application runs on. The environment brings its own libraries, so the
    /// smallest runtime with graphics drivers is enough.
    #[serde(default = "default_runtime")]
    pub runtime: String,
    #[serde(default = "default_runtime_version")]
    pub runtime_version: String,
    /// Permissions of the sandbox, see
    /// <https://docs.flatpak.org/en/latest/sandbox-permissions.html>.
    /// The default allows graphical applications with network access to open files in the
    /// home directory.
    #[serde(default = "default_finish_args")]
    pub finish_args: Vec<String>,
}

fn default_runtime() -> String {
    "org.freedesktop.Platform".to_string()
}

fn default_runtime_version() -> String {
    "25.08".to_string()
}

fn default_finish_args() -> Vec<String> {
    [
        "--share=ipc",
        "--share=network",
        "--socket=x11",
        "--socket=wayland",
        "--device=dri",
        "--filesystem=home",
    ]
    .map(String::from)
    .to_vec()
}

impl Default for AppFlatpakConfig {
    fn default() -> Self {
        Self {
            runtime: default_runtime(),
            runtime_version: default_runtime_version(),
            finish_args: default_finish_args(),
        }
    }
}

/// Whether the Flatpak tools are installed.
pub(super) fn check_host() -> Result<()> {
    if !cfg!(target_os = "linux") {
        bail!("Flatpak bundles can only be created on Linux");
    }
    if !super::which("flatpak") {
        bail!(
            "creating Flatpak bundles requires `flatpak` on the PATH, e.g. from `apt install flatpak`"
        );
    }
    Ok(())
}

/// What a Flatpak contains besides the application's desktop entry and icon.
pub(super) enum FlatpakContents {
    /// The environment, unpacked while building. The pack is moved into the build.
    Environment {
        pack_dir: PathBuf,
        pixi_unpack: Vec<u8>,
    },
    /// `pixi-launcher`, which unpacks the pack on first start.
    Launcher {
        pixi_launcher: Vec<u8>,
        pack_file: PathBuf,
        launcher_config: PathBuf,
    },
}

/// Create a Flatpak in `out_dir`.
///
/// This uses the `flatpak build-*` commands rather than `flatpak-builder`, as `flatpak-builder`
/// always commits the build to its cache, another uncompressed copy of the environment.
pub(super) fn create(
    app: &App,
    config: &AppFlatpakConfig,
    contents: FlatpakContents,
    platform: Platform,
    staging: &Path,
    out_dir: &Path,
) -> Result<PathBuf> {
    let arch = arch(platform)?;
    let id = flatpak_id(&app.identifier)?;
    let command_name = super::executable_name(&app.name, Platform::Linux64);

    // Lets `flatpak` install the runtime. Does nothing if Flathub was already added.
    run(Command::new("flatpak").args([
        "remote-add",
        "--user",
        "--if-not-exists",
        "flathub",
        FLATHUB_REPO,
    ]))?;
    run(Command::new("flatpak")
        .args(["install", "--user", "--noninteractive", "--or-update"])
        .arg(format!("--arch={arch}"))
        .arg("flathub")
        .arg(format!("{}//{}", config.runtime, config.runtime_version)))?;

    let build_dir = staging.join("flatpak-build");
    if build_dir.exists() {
        std::fs::remove_dir_all(&build_dir)?;
    }
    // Nothing is compiled, the runtime has everything unpacking needs and is used as SDK.
    run(Command::new("flatpak")
        .arg("build-init")
        .arg(format!("--arch={arch}"))
        .arg(&build_dir)
        .arg(&id)
        .arg(&config.runtime)
        .arg(&config.runtime)
        .arg(&config.runtime_version))?;

    // `files` is mounted at /app.
    let files = build_dir.join("files");
    let command = files.join("bin").join(&command_name);
    match contents {
        FlatpakContents::Environment {
            pack_dir,
            pixi_unpack,
        } => {
            // Unpacking extracts the packages next to the pack, so with the pack in /app the
            // environment hardlinks the extracted files instead of copying them.
            let (pack, unpack) = (files.join(".pack"), files.join(".pixi-unpack"));
            std::fs::rename(&pack_dir, &pack)
                .context("could not move the pack into the Flatpak build")?;
            write_file(&unpack, &pixi_unpack, 0o755)?;
            let mut unpack_command = Command::new("flatpak");
            unpack_command.arg("build").arg("--die-with-parent");
            // Packages left out of the pack are downloaded while building.
            if pack.join(REMOTE_PACKAGES_PATH).exists() {
                unpack_command.arg("--share=network");
            }
            run(unpack_command.arg(&build_dir).args([
                "/app/.pixi-unpack",
                "--output-directory",
                "/app",
                "--env-name",
                "env",
                "/app/.pack",
            ]))?;
            std::fs::remove_dir_all(&pack)?;
            std::fs::remove_file(&unpack)?;
            write_file(&command, launch_script(&app.command).as_bytes(), 0o755)?;
        }
        FlatpakContents::Launcher {
            pixi_launcher,
            pack_file,
            launcher_config,
        } => {
            // See `find_bundled_launcher_config`.
            let resources = files.join("share/pixi-launcher");
            std::fs::create_dir_all(&resources)?;
            std::fs::copy(&launcher_config, resources.join(LAUNCHER_CONFIG_FILE_NAME))?;
            std::fs::copy(&pack_file, resources.join(PACK_FILE_NAME))?;
            write_file(&command, &pixi_launcher, 0o755)?;
        }
    }

    write_file(
        &files.join(format!("share/applications/{id}.desktop")),
        desktop_entry(app, &id, &command_name, app.icon_png().is_some()).as_bytes(),
        0o644,
    )?;
    if let Some(icon) = app.icon_png() {
        let icons = files.join("share/icons/hicolor");
        std::fs::create_dir_all(&icons)?;
        let png = icons.join("icon.png");
        let size = prepare_icon(icon, &png)?;
        let apps = icons.join(format!("{size}x{size}/apps"));
        std::fs::create_dir_all(&apps)?;
        std::fs::rename(&png, apps.join(format!("{id}.png")))?;
    }

    run(Command::new("flatpak")
        .arg("build-finish")
        .arg(format!("--command={command_name}"))
        .args(&config.finish_args)
        .arg(&build_dir))?;
    let repo = staging.join("flatpak-repo");
    run(Command::new("flatpak")
        .arg("build-export")
        .arg(format!("--arch={arch}"))
        .arg(&repo)
        .arg(&build_dir)
        .arg(BRANCH))?;
    // The repo has a compressed copy now, the bundle is created from it.
    std::fs::remove_dir_all(&build_dir)?;

    std::fs::create_dir_all(out_dir)?;
    let bundle = out_dir.join(format!("{command_name}_{}_{arch}.flatpak", app.version));
    run(Command::new("flatpak")
        .arg("build-bundle")
        .arg(format!("--arch={arch}"))
        .arg(format!("--runtime-repo={FLATHUB_REPO}"))
        .arg(&repo)
        .arg(&bundle)
        .arg(&id)
        .arg(BRANCH))?;
    Ok(bundle)
}

fn write_file(path: &Path, contents: &[u8], mode: u32) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    Ok(())
}

fn run(command: &mut Command) -> Result<()> {
    tracing::info!("Running {:?}", command);
    let status = command
        .status()
        .with_context(|| format!("could not run {:?}", command.get_program()))?;
    if !status.success() {
        bail!("{:?} failed with {}", command.get_program(), status);
    }
    Ok(())
}

/// The Flatpak application id for a reverse-DNS identifier, which is already checked to consist
/// of letters, digits, `-` and `_`. Flatpak only allows `-` in the last part and needs at least
/// three parts, see <https://docs.flatpak.org/en/latest/conventions.html#application-ids>.
fn flatpak_id(identifier: &str) -> Result<String> {
    let parts: Vec<&str> = identifier.split('.').collect();
    if parts.len() < 3 {
        bail!(
            "a Flatpak needs an identifier with at least three parts like `org.example.app`, not `{identifier}`"
        );
    }
    if parts
        .iter()
        .any(|p| p.starts_with(|c: char| c.is_ascii_digit()))
    {
        bail!("the parts of a Flatpak identifier must not start with a digit: `{identifier}`");
    }
    let last = parts.len() - 1;
    Ok(parts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if i < last {
                p.replace('-', "_")
            } else {
                p.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("."))
}

fn arch(platform: Platform) -> Result<&'static str> {
    Ok(match platform {
        Platform::Linux64 => "x86_64",
        Platform::LinuxAarch64 => "aarch64",
        _ => bail!("Flatpak bundles are not supported for {}", platform),
    })
}

/// The command of the application: activate the environment, then start the command.
fn launch_script(command: &[String]) -> String {
    let placeholders = menu_placeholders(Path::new(PREFIX), Platform::Linux64);
    let command = command
        .iter()
        .map(|c| shell_quote(&replace_placeholders(c, &placeholders)))
        .collect::<Vec<_>>()
        .join(" ");
    format!("#!/bin/bash\nsource /app/activate.sh\nexec {command} \"$@\"\n")
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// <https://specifications.freedesktop.org/desktop-entry-spec/latest/>
fn desktop_entry(app: &App, id: &str, command_name: &str, has_icon: bool) -> String {
    // Line breaks would end the value early.
    let escape = |s: &str| {
        s.replace('\\', r"\\")
            .replace('\n', r"\n")
            .replace('\r', r"\r")
            .replace('\t', r"\t")
    };
    let linux = &app.linux;
    let exec = match &linux.field_code {
        Some(code) => format!("{command_name} {code}"),
        None => command_name.to_string(),
    };
    let mut entry = format!(
        "[Desktop Entry]\nType=Application\nName={}\nExec={exec}\nTerminal=false\n",
        escape(&app.name)
    );
    if let Some(description) = &app.description {
        entry.push_str(&format!("Comment={}\n", escape(description)));
    }
    if has_icon {
        entry.push_str(&format!("Icon={id}\n"));
    }
    if !linux.categories.is_empty() {
        entry.push_str(&format!("Categories={};\n", linux.categories.join(";")));
    }
    if !linux.mime_types.is_empty() {
        entry.push_str(&format!("MimeType={};\n", linux.mime_types.join(";")));
    }
    if let Some(class) = &linux.startup_wm_class {
        entry.push_str(&format!("StartupWMClass={}\n", escape(class)));
    }
    entry
}

/// Copy the icon to `destination`, scaled down to the largest size Flatpak exports.
/// Returns its size.
fn prepare_icon(icon: &Path, destination: &Path) -> Result<u32> {
    let mut image =
        image::open(icon).with_context(|| format!("could not read icon {}", icon.display()))?;
    if image.width() != image.height() {
        bail!("the icon {} has to be square", icon.display());
    }
    if image.width() > MAX_ICON_SIZE {
        image = image.resize(
            MAX_ICON_SIZE,
            MAX_ICON_SIZE,
            image::imageops::FilterType::Lanczos3,
        );
    }
    image
        .save_with_format(destination, image::ImageFormat::Png)
        .context("could not write icon")?;
    Ok(image.width())
}

/* --------------------------------------------------------------------------------------------- */
/*                                             TESTS                                             */
/* --------------------------------------------------------------------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App {
            name: "Ocean Viewer".to_string(),
            identifier: "org.example.ocean-viewer".to_string(),
            version: "1.0.0".to_string(),
            description: Some("Look at\nthe ocean".to_string()),
            command: vec!["{{ PYTHON }}".into(), "-m".into(), "it's".into()],
            icons: vec![],
            linux: super::super::menu::LinuxDesktop {
                categories: vec!["Science".to_string(), "Graphics".to_string()],
                field_code: Some("%F".to_string()),
                ..Default::default()
            },
        }
    }

    // Flatpaks are only built on Linux, elsewhere paths would be joined with `\`.
    #[cfg(unix)]
    #[test]
    fn test_launch_script() {
        assert_eq!(
            launch_script(&app().command),
            "#!/bin/bash\nsource /app/activate.sh\nexec '/app/env/bin/python' '-m' 'it'\\''s' \"$@\"\n"
        );
    }

    #[test]
    fn test_desktop_entry() {
        assert_eq!(
            desktop_entry(&app(), "org.example.ocean-viewer", "ocean-viewer", true),
            "[Desktop Entry]\nType=Application\nName=Ocean Viewer\nExec=ocean-viewer %F\nTerminal=false\n\
             Comment=Look at\\nthe ocean\nIcon=org.example.ocean-viewer\nCategories=Science;Graphics;\n"
        );
    }

    #[test]
    fn test_flatpak_id() {
        assert_eq!(
            flatpak_id("org.spyder-ide.Spyder-ide").unwrap(),
            "org.spyder_ide.Spyder-ide"
        );
        assert!(flatpak_id("org.example").is_err());
        assert!(flatpak_id("org.2d.viewer").is_err());
    }

    #[test]
    fn test_prepare_icon() {
        let dir = tempfile::tempdir().unwrap();
        let large = dir.path().join("large.png");
        image::RgbaImage::new(1024, 1024).save(&large).unwrap();
        let icon = dir.path().join("icon.png");
        assert_eq!(prepare_icon(&large, &icon).unwrap(), 512);
        assert_eq!(image::open(&icon).unwrap().width(), 512);

        let wide = dir.path().join("wide.png");
        image::RgbaImage::new(64, 32).save(&wide).unwrap();
        assert!(prepare_icon(&wide, &icon).is_err());
    }

    #[test]
    fn test_default_config() {
        let config: AppFlatpakConfig = toml::from_str("").unwrap();
        assert_eq!(config, AppFlatpakConfig::default());
        let config: AppFlatpakConfig =
            toml::from_str(r#"finish-args = ["--socket=wayland"]"#).unwrap();
        assert_eq!(config.finish_args, vec!["--socket=wayland"]);
        assert_eq!(config.runtime, "org.freedesktop.Platform");
    }
}
