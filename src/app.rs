//! Creating applications (macOS `.app`/`.dmg`, Windows installers, Linux AppImages and
//! Debian packages) from a pack.
//!
//! The application contains the `pixi-launcher` executable as its main binary, the pack and a
//! [`LauncherConfig`]. Unpacking happens on first launch, see [`crate::launch`].

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use cargo_packager::{
    PackageFormat,
    config::{Binary, ConfigBuilder, MacOsConfig, NsisConfig, Resource},
};
use rattler_conda_types::Platform;
use rattler_digest::{Sha256, compute_file_digest};
use rattler_lock::UrlOrPath;
use serde::Deserialize;

use crate::{
    LAUNCHER_CONFIG_FILE_NAME, LauncherConfig, PackOptions, pack::download_release_executable,
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
    /// Icon files (`.png`, `.icns` or `.ico`), relative to the config file.
    #[serde(default)]
    pub icons: Vec<PathBuf>,
    /// Which formats to create. Only the formats for the packed platform are created, the
    /// default is `dmg` on macOS, `nsis` on Windows and `appimage` on Linux.
    #[serde(default)]
    pub formats: Vec<AppFormat>,
    #[serde(default)]
    pub macos: AppMacOsConfig,
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AppFormat {
    /// macOS application bundle.
    App,
    /// macOS disk image.
    Dmg,
    /// Windows installer.
    Nsis,
    /// Linux AppImage.
    #[serde(rename = "appimage")]
    AppImage,
    /// Debian package.
    Deb,
}

impl AppFormat {
    fn is_for(self, platform: Platform) -> bool {
        match self {
            AppFormat::App | AppFormat::Dmg => platform.is_osx(),
            AppFormat::Nsis => platform.is_windows(),
            AppFormat::AppImage | AppFormat::Deb => platform.is_linux(),
        }
    }

    fn default_for(platform: Platform) -> Result<Self> {
        if platform.is_osx() {
            Ok(AppFormat::Dmg)
        } else if platform.is_windows() {
            Ok(AppFormat::Nsis)
        } else if platform.is_linux() {
            Ok(AppFormat::AppImage)
        } else {
            bail!("creating applications is not supported for {}", platform)
        }
    }

    /// Check that the format can be created on the current host.
    fn check_host(self) -> Result<()> {
        let supported = match self {
            AppFormat::App | AppFormat::Dmg => cfg!(target_os = "macos"),
            // Downloaded automatically on Windows, elsewhere it has to be installed.
            AppFormat::Nsis => cfg!(windows) || which("makensis"),
            AppFormat::AppImage | AppFormat::Deb => cfg!(target_os = "linux"),
        };
        if supported {
            return Ok(());
        }
        match self {
            AppFormat::App | AppFormat::Dmg => bail!("{:?} can only be created on macOS", self),
            AppFormat::Nsis => bail!(
                "creating Windows installers on this platform requires `makensis` on the PATH, e.g. from the conda-forge `nsis` package"
            ),
            AppFormat::AppImage | AppFormat::Deb => {
                bail!("{:?} can only be created on Linux", self)
            }
        }
    }

    fn package_format(self) -> PackageFormat {
        match self {
            AppFormat::App => PackageFormat::App,
            AppFormat::Dmg => PackageFormat::Dmg,
            AppFormat::Nsis => PackageFormat::Nsis,
            AppFormat::AppImage => PackageFormat::AppImage,
            AppFormat::Deb => PackageFormat::Deb,
        }
    }
}

fn which(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
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

    /// The formats to create for `platform`.
    fn formats_for(&self, platform: Platform) -> Result<Vec<AppFormat>> {
        if self.formats.is_empty() {
            return Ok(vec![AppFormat::default_for(platform)?]);
        }
        let formats: Vec<AppFormat> = self
            .formats
            .iter()
            .copied()
            .filter(|f| f.is_for(platform))
            .collect();
        if formats.is_empty() {
            bail!(
                "none of the configured formats {:?} can be created for {}",
                self.formats,
                platform
            );
        }
        Ok(formats)
    }
}

/// Pack the environment and bundle it as an application in `out_dir`.
/// `launcher_source` is a path or URL to the `pixi-launcher` executable, by default it is
/// downloaded from the pixi-pack release.
/// Returns the paths of the created files.
pub async fn create_app(
    pack_options: PackOptions,
    app: AppConfig,
    launcher_source: Option<UrlOrPath>,
    out_dir: &Path,
) -> Result<Vec<PathBuf>> {
    let platform = resolve_platform(
        &pack_options.manifest_path,
        &pack_options.environment,
        &pack_options.platform,
    )?;
    let formats = app.formats_for(platform)?;
    for format in &formats {
        format.check_host()?;
    }
    if formats.contains(&AppFormat::AppImage)
        && !app
            .icons
            .iter()
            .any(|i| i.extension().is_some_and(|e| e == "png"))
    {
        bail!("an AppImage requires a `.png` icon");
    }

    let staging = tempfile::tempdir().context("could not create temporary directory")?;
    let staging = staging.path();

    let pack_file = staging.join(PACK_FILE_NAME);
    crate::pack(PackOptions {
        output_file: pack_file.clone(),
        output_mode: crate::OutputMode::Archive,
        ..pack_options
    })
    .await?;

    let launcher_config_file = staging.join(LAUNCHER_CONFIG_FILE_NAME);
    write_launcher_config(&app, &pack_file, &launcher_config_file)?;

    let executable_name = executable_name(&app.name, platform);
    let executable =
        download_release_executable("pixi-launcher", launcher_source, platform).await?;
    // cargo-packager expects the binary path without the `.exe` extension.
    let main_binary = staging.join(&executable_name);
    let main_binary_file = if platform.is_windows() {
        staging.join(format!("{executable_name}.exe"))
    } else {
        main_binary.clone()
    };
    std::fs::write(&main_binary_file, executable)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&main_binary_file, std::fs::Permissions::from_mode(0o755))?;
    }

    let mut macos = MacOsConfig::new();
    if let Some(identity) = &app.macos.signing_identity {
        macos = macos.signing_identity(identity);
    }
    if let Some(version) = &app.macos.minimum_system_version {
        macos = macos.minimum_system_version(version);
    }
    // Lets the uninstaller remove the unpacked environment, see `LauncherConfig::app_data_dir`.
    let nsis = NsisConfig::new().appdata_paths(["$LOCALAPPDATA\\$IDENTIFIER"]);

    let resource = |src: &Path, target: &str| Resource::Mapped {
        src: src.to_string_lossy().into_owned(),
        target: PathBuf::from(target),
    };
    let config = ConfigBuilder::new()
        .product_name(&app.name)
        .identifier(&app.identifier)
        .version(&app.version)
        .target_triple(target_triple(platform)?)
        .binaries([Binary::new(&main_binary).main(true)])
        .resources([
            resource(&pack_file, PACK_FILE_NAME),
            resource(&launcher_config_file, LAUNCHER_CONFIG_FILE_NAME),
        ])
        .icons(app.icons.iter().map(|i| i.to_string_lossy().into_owned()))
        .formats(formats.iter().map(|f| f.package_format()))
        .out_dir(out_dir)
        .macos(macos)
        .nsis(nsis)
        .config()
        .clone();

    eprintln!("📦 Creating application for {}", app.name);
    let outputs = tokio::task::spawn_blocking(move || cargo_packager::package(&config))
        .await?
        .map_err(|e| anyhow!("could not create application: {}", e))?;

    // cargo-packager keeps downloaded tools next to the output, and always creates the `.app`
    // for a `.dmg`. Only leave behind what was asked for.
    let _ = std::fs::remove_dir_all(out_dir.join(".cargo-packager"));
    let mut paths = Vec::new();
    for output in outputs {
        if output.format == PackageFormat::App && !formats.contains(&AppFormat::App) {
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

fn write_launcher_config(app: &AppConfig, pack_file: &Path, destination: &Path) -> Result<()> {
    let pack_sha256 = compute_file_digest::<Sha256>(pack_file)?
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
    std::fs::write(destination, serde_json::to_string_pretty(&launcher_config)?)?;
    Ok(())
}

/// cargo-packager derives the architecture of the packages from the target triple.
fn target_triple(platform: Platform) -> Result<&'static str> {
    Ok(match platform {
        Platform::OsxArm64 => "aarch64-apple-darwin",
        Platform::Osx64 => "x86_64-apple-darwin",
        Platform::Win64 => "x86_64-pc-windows-msvc",
        Platform::WinArm64 => "aarch64-pc-windows-msvc",
        Platform::Linux64 => "x86_64-unknown-linux-gnu",
        Platform::LinuxAarch64 => "aarch64-unknown-linux-gnu",
        _ => bail!("creating applications is not supported for {}", platform),
    })
}

/// A file name for the main executable, which shows up e.g. in Activity Monitor or the Task
/// Manager. cargo-packager strips everything after a `.`. On Linux, it is also the name of the
/// command and of the Debian package, so it is lowercase without spaces.
fn executable_name(app_name: &str, platform: Platform) -> String {
    let name: String = app_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if platform.is_linux() {
        name.to_lowercase().replace([' ', '_'], "-")
    } else {
        name
    }
}

/* --------------------------------------------------------------------------------------------- */
/*                                             TESTS                                             */
/* --------------------------------------------------------------------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;

    fn write_config(contents: &str) -> (tempfile::TempDir, Result<AppConfig>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.toml");
        std::fs::write(&path, contents).unwrap();
        let config = AppConfig::from_file(&path);
        (dir, config)
    }

    #[test]
    fn test_app_config_defaults() {
        let (dir, config) = write_config(
            r#"
            name = "Ocean Viewer"
            identifier = "org.example.ocean-viewer"
            version = "1.0.0"
            entry-point = "napari"
            icons = ["icon.png"]
            "#,
        );
        let config = config.unwrap();
        assert_eq!(config.icons, vec![dir.path().join("icon.png")]);
        assert_eq!(config.macos, AppMacOsConfig::default());
        assert_eq!(
            config.formats_for(Platform::OsxArm64).unwrap(),
            vec![AppFormat::Dmg]
        );
        assert_eq!(
            config.formats_for(Platform::Win64).unwrap(),
            vec![AppFormat::Nsis]
        );
        assert_eq!(
            config.formats_for(Platform::Linux64).unwrap(),
            vec![AppFormat::AppImage]
        );
    }

    #[test]
    fn test_formats_are_filtered_by_platform() {
        let (_dir, config) = write_config(
            r#"
            name = "x"
            identifier = "x"
            version = "1"
            entry-point = "x"
            formats = ["app", "dmg", "nsis", "deb"]
            "#,
        );
        let config = config.unwrap();
        assert_eq!(
            config.formats_for(Platform::Osx64).unwrap(),
            vec![AppFormat::App, AppFormat::Dmg]
        );
        assert_eq!(
            config.formats_for(Platform::LinuxAarch64).unwrap(),
            vec![AppFormat::Deb]
        );
        let (_dir, config) = write_config(
            r#"
            name = "x"
            identifier = "x"
            version = "1"
            entry-point = "x"
            formats = ["dmg"]
            "#,
        );
        assert!(config.unwrap().formats_for(Platform::Win64).is_err());
    }

    #[test]
    fn test_executable_name() {
        assert_eq!(
            executable_name("Ocean Viewer", Platform::OsxArm64),
            "Ocean Viewer"
        );
        assert_eq!(
            executable_name("Viewer 2.0/beta", Platform::Win64),
            "Viewer 2_0_beta"
        );
        assert_eq!(
            executable_name("Ocean Viewer 2.0", Platform::Linux64),
            "ocean-viewer-2-0"
        );
    }

    #[test]
    fn test_app_config_rejects_unknown_fields() {
        let (_dir, config) = write_config(
            r#"
            name = "x"
            identifier = "x"
            version = "1"
            entry-point = "x"
            entrypoint = "typo"
            "#,
        );
        assert!(config.is_err());
    }
}
