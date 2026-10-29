//! Creating application bundles (macOS `.app` and `.dmg`) from a pack.
//!
//! The bundle contains the `pixi-unpack` executable as its main binary, the pack and a
//! [`LauncherConfig`]. Unpacking happens on first launch, see [`crate::launch`].

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use cargo_packager::{
    PackageFormat,
    config::{Binary, ConfigBuilder, MacOsConfig, Resource},
};
use rattler_digest::{Sha256, compute_file_digest};
use serde::Deserialize;

use crate::{
    LAUNCHER_CONFIG_FILE_NAME, LauncherConfig, PackOptions, pack::download_pixi_unpack_executable,
    resolve_platform,
};

const PACK_FILE_NAME: &str = "pack.tar";

/// Application settings, read from a TOML file passed to `--create-app`.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct AppConfig {
    /// Human readable name, e.g. `Ocean Viewer`.
    pub name: String,
    /// Reverse-DNS identifier, e.g. `org.example.ocean-viewer`.
    pub identifier: String,
    pub version: String,
    /// Executable in the environment to start, e.g. `napari`.
    pub entry_point: String,
    /// Arguments always passed to the entry point.
    #[serde(default)]
    pub args: Vec<String>,
    /// Icon files (`.png` or `.icns`), relative to the config file.
    #[serde(default)]
    pub icons: Vec<PathBuf>,
    /// Which bundles to create.
    #[serde(default = "default_formats")]
    pub formats: Vec<AppFormat>,
    #[serde(default)]
    pub macos: AppMacOsConfig,
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AppFormat {
    App,
    Dmg,
}

fn default_formats() -> Vec<AppFormat> {
    vec![AppFormat::Dmg]
}

#[derive(Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct AppMacOsConfig {
    /// Codesigning identity, e.g. `Developer ID Application: Jane Doe (TEAMID)`.
    /// Notarization runs when `APPLE_ID`, `APPLE_PASSWORD` and `APPLE_TEAM_ID`
    /// (or `APPLE_API_KEY`, `APPLE_API_ISSUER` and `APPLE_API_KEY_PATH`) are set.
    pub signing_identity: Option<String>,
    pub minimum_system_version: Option<String>,
}

impl AppConfig {
    /// Read the config, resolving relative icon paths against the config file.
    pub fn from_file(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("could not read app config {}", path.display()))?;
        let mut config: Self = toml::from_str(&contents)
            .with_context(|| format!("could not parse app config {}", path.display()))?;
        let base = path.parent().unwrap_or(Path::new("."));
        for icon in &mut config.icons {
            *icon = base.join(&*icon);
        }
        Ok(config)
    }
}

/// Pack the environment and bundle it as an application in `out_dir`.
/// Returns the paths of the created bundles.
pub async fn create_app(
    mut pack_options: PackOptions,
    app: AppConfig,
    out_dir: &Path,
) -> Result<Vec<PathBuf>> {
    if !cfg!(target_os = "macos") {
        bail!("creating applications is currently only supported on macOS");
    }
    let platform = resolve_platform(
        &pack_options.manifest_path,
        &pack_options.environment,
        &pack_options.platform,
    )?;
    if !platform.is_osx() {
        bail!(
            "creating applications is currently only supported for macOS platforms, not {}",
            platform
        );
    }

    let staging = tempfile::tempdir().context("could not create temporary directory")?;
    let staging = staging.path();

    let pack_file = staging.join(PACK_FILE_NAME);
    let pixi_unpack_source = pack_options.pixi_unpack_source.take();
    pack_options.output_file = pack_file.clone();
    pack_options.output_mode = crate::OutputMode::Archive;
    crate::pack(pack_options).await?;

    let pack_sha256 = compute_file_digest::<Sha256>(&pack_file)?
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let launcher_config = LauncherConfig {
        name: app.name.clone(),
        identifier: app.identifier.clone(),
        version: app.version.clone(),
        entry_point: app.entry_point.clone(),
        args: app.args.clone(),
        pack_file: PathBuf::from(PACK_FILE_NAME),
        pack_sha256,
    };
    let launcher_config_file = staging.join(LAUNCHER_CONFIG_FILE_NAME);
    std::fs::write(
        &launcher_config_file,
        serde_json::to_string_pretty(&launcher_config)?,
    )?;

    // The executable name shows up e.g. in Activity Monitor.
    let main_binary = staging.join(executable_name(&app.name));
    let executable = download_pixi_unpack_executable(pixi_unpack_source, platform).await?;
    std::fs::write(&main_binary, executable)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&main_binary, std::fs::Permissions::from_mode(0o755))?;
    }

    let mut macos = MacOsConfig::new();
    if let Some(identity) = &app.macos.signing_identity {
        macos = macos.signing_identity(identity);
    }
    if let Some(version) = &app.macos.minimum_system_version {
        macos = macos.minimum_system_version(version);
    }

    let resource = |src: &Path, target: &str| Resource::Mapped {
        src: src.to_string_lossy().into_owned(),
        target: PathBuf::from(target),
    };
    let config = ConfigBuilder::new()
        .product_name(&app.name)
        .identifier(&app.identifier)
        .version(&app.version)
        .binaries([Binary::new(&main_binary).main(true)])
        .resources([
            resource(&pack_file, PACK_FILE_NAME),
            resource(&launcher_config_file, LAUNCHER_CONFIG_FILE_NAME),
        ])
        .icons(app.icons.iter().map(|i| i.to_string_lossy().into_owned()))
        .formats(app.formats.iter().map(|f| match f {
            AppFormat::App => PackageFormat::App,
            AppFormat::Dmg => PackageFormat::Dmg,
        }))
        .out_dir(out_dir)
        .macos(macos)
        .config()
        .clone();

    eprintln!("📦 Creating application bundle for {}", app.name);
    let outputs = tokio::task::spawn_blocking(move || cargo_packager::package(&config))
        .await?
        .map_err(|e| anyhow!("could not create application bundle: {}", e))?;

    // cargo-packager keeps downloaded tools next to the output, and always creates the `.app`
    // for a `.dmg`. Only leave behind what was asked for.
    let _ = std::fs::remove_dir_all(out_dir.join(".cargo-packager"));
    let mut paths = Vec::new();
    for output in outputs {
        if output.format == PackageFormat::App && !app.formats.contains(&AppFormat::App) {
            for path in &output.paths {
                std::fs::remove_dir_all(path)?;
            }
        } else {
            paths.extend(output.paths);
        }
    }
    for path in &paths {
        eprintln!("📦 Created {}", path.display());
    }
    Ok(paths)
}

/// A file name for the main executable. cargo-packager strips everything after a `.`.
fn executable_name(app_name: &str) -> String {
    app_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/* --------------------------------------------------------------------------------------------- */
/*                                             TESTS                                             */
/* --------------------------------------------------------------------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_config_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.toml");
        std::fs::write(
            &path,
            r#"
            name = "Ocean Viewer"
            identifier = "org.example.ocean-viewer"
            version = "1.0.0"
            entry-point = "napari"
            icons = ["icon.png"]
            "#,
        )
        .unwrap();
        let config = AppConfig::from_file(&path).unwrap();
        assert_eq!(config.formats, vec![AppFormat::Dmg]);
        assert_eq!(config.icons, vec![dir.path().join("icon.png")]);
        assert_eq!(config.macos, AppMacOsConfig::default());
    }

    #[test]
    fn test_executable_name() {
        assert_eq!(executable_name("Ocean Viewer"), "Ocean Viewer");
        assert_eq!(executable_name("Viewer 2.0/beta"), "Viewer 2_0_beta");
    }

    #[test]
    fn test_app_config_rejects_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.toml");
        std::fs::write(
            &path,
            r#"
            name = "x"
            identifier = "x"
            version = "1"
            entry-point = "x"
            entrypoint = "typo"
            "#,
        )
        .unwrap();
        assert!(AppConfig::from_file(&path).is_err());
    }
}
