//! Creating a Flatpak bundle (`.flatpak`) with `flatpak-builder`.
//!
//! Flatpak applications always live at `/app` inside the sandbox, so unlike the other formats the
//! environment is unpacked at build time, by running `pixi-unpack` inside the build sandbox. The
//! application then starts the command directly from `/app/env`.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use rattler_conda_types::Platform;
use serde::Deserialize;
use serde_json::json;

use super::App;
use crate::{menu_placeholders, replace_placeholders};

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
    for program in ["flatpak", "flatpak-builder"] {
        if !super::which(program) {
            bail!(
                "creating Flatpak bundles requires `{program}` on the PATH, e.g. from `apt install flatpak flatpak-builder`"
            );
        }
    }
    Ok(())
}

/// Bundle the pack in `pack_dir` as a Flatpak in `out_dir`.
pub(super) fn create(
    app: &App,
    config: &AppFlatpakConfig,
    pack_dir: &Path,
    pixi_unpack: &[u8],
    platform: Platform,
    staging: &Path,
    out_dir: &Path,
) -> Result<PathBuf> {
    let arch = arch(platform)?;
    let command_name = super::executable_name(&app.name, Platform::Linux64);

    let files = staging.join("flatpak-files");
    std::fs::create_dir_all(&files)?;
    std::fs::write(files.join("pixi-unpack"), pixi_unpack)?;
    std::fs::write(files.join("launch.sh"), launch_script(&app.command))?;
    std::fs::write(
        files.join("app.desktop"),
        desktop_entry(app, &command_name, app.icon_png().is_some()),
    )?;

    let mut build_commands = vec![
        "chmod +x pixi-unpack".to_string(),
        "./pixi-unpack --output-directory /app --env-name env pack".to_string(),
        format!("install -Dm755 launch.sh /app/bin/{command_name}"),
        format!(
            "install -Dm644 app.desktop /app/share/applications/{}.desktop",
            app.identifier
        ),
    ];
    if let Some(icon) = app.icon_png() {
        let size = prepare_icon(icon, &files.join("icon.png"))?;
        build_commands.push(format!(
            "install -Dm644 icon.png /app/share/icons/hicolor/{size}x{size}/apps/{}.png",
            app.identifier
        ));
    }

    let manifest = json!({
        "id": app.identifier,
        "runtime": config.runtime,
        "runtime-version": config.runtime_version,
        // Nothing is compiled, the runtime has everything the build commands need.
        "sdk": config.runtime,
        "command": command_name,
        "finish-args": config.finish_args,
        "modules": [{
            "name": "environment",
            "buildsystem": "simple",
            // Stripping or splitting off debug info would modify the conda packages' binaries.
            "build-options": { "strip": false, "no-debuginfo": true },
            "build-commands": build_commands,
            "sources": [
                { "type": "dir", "path": pack_dir, "dest": "pack" },
                { "type": "dir", "path": files },
            ],
        }],
    });
    let manifest_path = staging.join(format!("{}.json", app.identifier));
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;

    // Lets `flatpak-builder` install the runtime. Does nothing if Flathub was already added.
    run(Command::new("flatpak").args([
        "remote-add",
        "--user",
        "--if-not-exists",
        "flathub",
        FLATHUB_REPO,
    ]))?;
    let repo = staging.join("flatpak-repo");
    run(Command::new("flatpak-builder")
        .args([
            "--user",
            "--install-deps-from=flathub",
            "--force-clean",
            // Needs FUSE, which e.g. containers often lack, and only speeds up large builds.
            "--disable-rofiles-fuse",
        ])
        .arg(format!("--arch={arch}"))
        .arg(format!("--default-branch={BRANCH}"))
        .arg("--state-dir")
        .arg(staging.join("flatpak-builder"))
        .arg("--repo")
        .arg(&repo)
        .arg(staging.join("flatpak-build"))
        .arg(&manifest_path))?;

    std::fs::create_dir_all(out_dir)?;
    let bundle = out_dir.join(format!("{command_name}_{}_{arch}.flatpak", app.version));
    run(Command::new("flatpak")
        .arg("build-bundle")
        .arg(format!("--arch={arch}"))
        .arg(format!("--runtime-repo={FLATHUB_REPO}"))
        .arg(&repo)
        .arg(&bundle)
        .arg(&app.identifier)
        .arg(BRANCH))?;
    Ok(bundle)
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
fn desktop_entry(app: &App, command_name: &str, has_icon: bool) -> String {
    // Newlines would end the value early.
    let escape = |s: &str| s.replace('\\', r"\\").replace('\n', r"\n");
    let exec = match &app.field_code {
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
        entry.push_str(&format!("Icon={}\n", app.identifier));
    }
    if !app.categories.is_empty() {
        entry.push_str(&format!("Categories={};\n", app.categories.join(";")));
    }
    if !app.mime_types.is_empty() {
        entry.push_str(&format!("MimeType={};\n", app.mime_types.join(";")));
    }
    if let Some(class) = &app.startup_wm_class {
        entry.push_str(&format!("StartupWMClass={class}\n"));
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
            categories: vec!["Science".to_string(), "Graphics".to_string()],
            startup_wm_class: None,
            mime_types: vec![],
            field_code: Some("%F".to_string()),
        }
    }

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
            desktop_entry(&app(), "ocean-viewer", true),
            "[Desktop Entry]\nType=Application\nName=Ocean Viewer\nExec=ocean-viewer %F\nTerminal=false\n\
             Comment=Look at\\nthe ocean\nIcon=org.example.ocean-viewer\nCategories=Science;Graphics;\n"
        );
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
