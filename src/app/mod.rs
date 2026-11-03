//! Creating applications (macOS `.app`/`.dmg`, Windows installers, Linux Flatpaks) from a pack.
//!
//! On macOS and Windows, the application contains the `pixi-launcher` executable as its main
//! binary, the pack and a [`LauncherConfig`]. Unpacking happens on first launch, see
//! [`crate::launch`]. Flatpaks are unpacked at build time, see [`flatpak`].

mod flatpak;
mod menu;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use cargo_packager::{
    PackageFormat,
    config::{Binary, ConfigBuilder, MacOsConfig, NsisConfig, Resource, WindowsConfig},
};
use rattler_conda_types::Platform;
use rattler_digest::{Sha256, compute_file_digest};
use rattler_lock::UrlOrPath;
use serde::Deserialize;

use self::flatpak::AppFlatpakConfig;
use crate::{
    LAUNCHER_CONFIG_FILE_NAME, LauncherConfig, PackOptions, REMOTE_PACKAGES_PATH,
    pack::{create_tarball, download_release_executable},
    resolve_platform,
};

const PACK_FILE_NAME: &str = "pack.tar";
const AD_HOC_SIGNING_IDENTITY: &str = "-";

/// Application settings, read from a TOML file passed to `--create-app`.
///
/// The application is either described by the menuinst shortcut (`Menu/*.json`) of a package in
/// the environment (`menu`), or by `name`, `identifier`, `version` and `entry-point`. Settings
/// given here override the ones of the shortcut.
#[derive(Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct AppConfig {
    /// Package whose menuinst shortcut describes the application, e.g. `napari`.
    pub menu: Option<String>,
    /// Name of the shortcut to use, if the package has several.
    pub menu_item: Option<String>,
    /// Human readable name, e.g. `Ocean Viewer`.
    pub name: Option<String>,
    /// Reverse-DNS identifier, e.g. `org.example.ocean-viewer`. Defaults to the macOS
    /// `CFBundleIdentifier` of the shortcut.
    pub identifier: Option<String>,
    /// Defaults to the version of the `menu` package.
    pub version: Option<String>,
    pub description: Option<String>,
    /// Executable in the environment to start, e.g. `napari`, instead of the shortcut's command.
    pub entry_point: Option<String>,
    /// Arguments always passed to the entry point.
    #[serde(default)]
    pub args: Vec<String>,
    /// Icon files (`.png`, `.icns` or `.ico`), relative to the config file. The shortcut's icon
    /// is used if none matches the platform.
    #[serde(default)]
    pub icons: Vec<PathBuf>,
    /// Which formats to create. Only the formats for the packed platform are created, the
    /// default is `dmg` on macOS, `nsis` on Windows and `flatpak` on Linux.
    #[serde(default)]
    pub formats: Vec<AppFormat>,
    /// Download conda packages from their channels on first launch instead of including them in
    /// the application, which keeps it small. Packages without an `http(s)` URL are still
    /// included. Flatpaks then contain `pixi-launcher` and unpack the environment into the
    /// application's data directory, like on the other platforms.
    #[serde(default)]
    pub download_packages: bool,
    #[serde(default)]
    pub macos: AppMacOsConfig,
    #[serde(default)]
    pub windows: AppWindowsConfig,
    #[serde(default)]
    pub flatpak: AppFlatpakConfig,
}

/// The application to create, from the [`AppConfig`] and the shortcut it refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct App {
    name: String,
    identifier: String,
    version: String,
    description: Option<String>,
    /// See [`LauncherConfig::command`].
    command: Vec<String>,
    icons: Vec<PathBuf>,
    linux: menu::LinuxDesktop,
}

impl App {
    fn icon_png(&self) -> Option<&Path> {
        self.icons
            .iter()
            .find(|i| i.extension().is_some_and(|e| e == "png"))
            .map(PathBuf::as_path)
    }
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
    /// Linux Flatpak bundle.
    Flatpak,
}

impl AppFormat {
    fn is_for(self, platform: Platform) -> bool {
        match self {
            AppFormat::App | AppFormat::Dmg => platform.is_osx(),
            AppFormat::Nsis => platform.is_windows(),
            AppFormat::Flatpak => platform.is_linux(),
        }
    }

    fn default_for(platform: Platform) -> Result<Self> {
        if platform.is_osx() {
            Ok(AppFormat::Dmg)
        } else if platform.is_windows() {
            Ok(AppFormat::Nsis)
        } else if platform.is_linux() {
            Ok(AppFormat::Flatpak)
        } else {
            bail!("creating applications is not supported for {}", platform)
        }
    }

    /// Check that the format can be created on the current host.
    fn check_host(self) -> Result<()> {
        match self {
            AppFormat::App | AppFormat::Dmg if !cfg!(target_os = "macos") => {
                bail!("{:?} can only be created on macOS", self)
            }
            // Downloaded automatically on Windows, elsewhere it has to be installed.
            AppFormat::Nsis if !cfg!(windows) && !which("makensis") => bail!(
                "creating Windows installers on this platform requires `makensis` on the PATH, e.g. from the conda-forge `nsis` package"
            ),
            AppFormat::Flatpak => flatpak::check_host(),
            _ => Ok(()),
        }
    }

    fn package_format(self) -> PackageFormat {
        match self {
            AppFormat::App => PackageFormat::App,
            AppFormat::Dmg => PackageFormat::Dmg,
            AppFormat::Nsis => PackageFormat::Nsis,
            AppFormat::Flatpak => unreachable!("Flatpaks are not created with cargo-packager"),
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
    /// Without it, the application is signed ad-hoc.
    pub signing_identity: Option<String>,
    pub minimum_system_version: Option<String>,
}

/// Code signing of the launcher, the installer and the uninstaller on Windows.
#[derive(Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct AppWindowsConfig {
    /// SHA1 thumbprint of a certificate in the Windows certificate store, used with `signtool`.
    /// Only works on Windows.
    pub certificate_thumbprint: Option<String>,
    /// Timestamp server for `signtool`, e.g. `http://timestamp.digicert.com`.
    pub timestamp_url: Option<String>,
    /// Whether the timestamp server uses RFC 3161 (`/tr`) instead of Authenticode (`/t`).
    #[serde(default)]
    pub tsp: bool,
    /// A command to sign a file instead of `signtool`, which also works on other platforms,
    /// e.g. with `osslsigncode` or a script. `%1` is replaced by the path of the file to sign.
    /// The command is split on spaces.
    pub sign_command: Option<String>,
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

    /// Combine the config with the shortcut of the `menu` package, if any.
    fn resolve(&self, pack_dir: &Path, platform: Platform, extract_dir: &Path) -> Result<App> {
        let menu = self
            .menu
            .as_deref()
            .map(|package| {
                menu::load(
                    pack_dir,
                    platform,
                    package,
                    self.menu_item.as_deref(),
                    extract_dir,
                )
            })
            .transpose()?;
        let missing = |field: &str| {
            anyhow!(
                "the app config needs `{field}`, or `menu` with a package that has a menuinst shortcut"
            )
        };
        let menu_field = |f: fn(&menu::MenuApp) -> Option<String>| menu.as_ref().and_then(f);

        let command = match (&self.entry_point, &menu) {
            (Some(entry_point), _) => std::iter::once(entry_point.clone())
                .chain(self.args.iter().cloned())
                .collect(),
            (None, Some(menu)) if self.args.is_empty() => menu.command.clone(),
            (None, Some(_)) => bail!("`args` can only be used with `entry-point`"),
            (None, None) => return Err(missing("entry-point")),
        };
        let mut icons = self.icons.clone();
        if let Some(icon) = menu.as_ref().and_then(|m| m.icon.clone()) {
            let ext = icon.extension();
            if !icons.iter().any(|i| i.extension() == ext) {
                icons.push(icon);
            }
        }
        let identifier = self
            .identifier
            .clone()
            .or_else(|| menu_field(|m| m.identifier.clone()))
            .ok_or_else(|| missing("identifier"))?;
        validate_identifier(&identifier)?;
        Ok(App {
            name: self
                .name
                .clone()
                .or_else(|| menu_field(|m| Some(m.name.clone())))
                .ok_or_else(|| missing("name"))?,
            identifier,
            version: self
                .version
                .clone()
                .or_else(|| menu_field(|m| Some(m.version.clone())))
                .ok_or_else(|| missing("version"))?,
            description: self
                .description
                .clone()
                .or_else(|| menu_field(|m| Some(m.description.clone()))),
            command,
            icons,
            linux: menu.map(|m| m.linux).unwrap_or_default(),
        })
    }
}

/// The identifier names the data directory and the Flatpak, so it has to be reverse-DNS.
fn validate_identifier(identifier: &str) -> Result<()> {
    let segments: Vec<&str> = identifier.split('.').collect();
    let valid_segment = |s: &&str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    };
    if segments.len() < 2 || !segments.iter().all(valid_segment) {
        bail!(
            "the identifier `{identifier}` has to be in reverse-DNS notation like `org.example.app`, using letters, digits, `-` and `_`"
        );
    }
    Ok(())
}

/// A version cargo-packager accepts: the installers require `MAJOR.MINOR.PATCH`, while conda
/// versions are often like `2024.05`, `1.2.3.post1` or `6.0.0a1`.
fn numeric_version(version: &str) -> String {
    let mut parts: Vec<u64> = version
        .split('.')
        .map_while(|part| part.parse().ok())
        .take(3)
        .collect();
    parts.resize(3, 0);
    parts
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(".")
}

/// Pack the environment and bundle it as an application in `out_dir`.
/// `launcher_source` is a path or URL to the `pixi-launcher` executable, by default it is
/// downloaded from the pixi-pack release. Flatpaks use `pixi-unpack` instead, from
/// `pack_options.pixi_unpack_source`.
/// Returns the paths of the created files.
pub async fn create_app(
    pack_options: PackOptions,
    config: AppConfig,
    launcher_source: Option<UrlOrPath>,
    out_dir: &Path,
) -> Result<Vec<PathBuf>> {
    let platform = resolve_platform(
        &pack_options.manifest_path,
        &pack_options.environment,
        &pack_options.platform,
    )?;
    let formats = config.formats_for(platform)?;
    for format in &formats {
        format.check_host()?;
    }
    // Fail before packing, which takes a while. A shortcut's identifier is checked later.
    if let Some(identifier) = &config.identifier {
        validate_identifier(identifier)?;
    }

    let staging = tempfile::tempdir().context("could not create temporary directory")?;
    let staging = staging.path();

    let pack_dir = staging.join("pack");
    let pixi_unpack_source = pack_options.pixi_unpack_source.clone();
    crate::pack(PackOptions {
        output_file: pack_dir.clone(),
        output_mode: crate::OutputMode::DirectoryOnly,
        download_packages: config.download_packages,
        ..pack_options
    })
    .await?;
    let app = config.resolve(&pack_dir, platform, &staging.join("menu-package"))?;

    eprintln!("📦 Creating application for {}", app.name);
    let paths = if formats.contains(&AppFormat::Flatpak) {
        let contents = if pack_dir.join(REMOTE_PACKAGES_PATH).exists() {
            // Like on the other platforms, the launcher downloads the packages on first start.
            let pack_file = staging.join(PACK_FILE_NAME);
            create_tarball(&pack_dir, &pack_file)?;
            let launcher_config = staging.join(LAUNCHER_CONFIG_FILE_NAME);
            write_launcher_config(&app, &pack_file, &launcher_config)?;
            flatpak::FlatpakContents::Launcher {
                pixi_launcher: download_release_executable(
                    "pixi-launcher",
                    launcher_source,
                    platform,
                )
                .await?,
                pack_file,
                launcher_config,
            }
        } else {
            flatpak::FlatpakContents::Environment {
                pack_dir: pack_dir.clone(),
                pixi_unpack: download_release_executable(
                    "pixi-unpack",
                    pixi_unpack_source,
                    platform,
                )
                .await?,
            }
        };
        let flatpak = tokio::task::spawn_blocking({
            let (app, staging, out_dir) =
                (app.clone(), staging.to_path_buf(), out_dir.to_path_buf());
            move || {
                flatpak::create(
                    &app,
                    &config.flatpak,
                    contents,
                    platform,
                    &staging,
                    &out_dir,
                )
            }
        })
        .await??;
        vec![flatpak]
    } else {
        let pack_file = staging.join(PACK_FILE_NAME);
        create_tarball(&pack_dir, &pack_file)?;
        create_packaged_app(
            &app,
            &config,
            &formats,
            platform,
            &pack_file,
            launcher_source,
            staging,
            out_dir,
        )
        .await?
    };
    for path in &paths {
        eprintln!("📦 Created {}", path.display());
    }
    Ok(paths)
}

/// Create the macOS and Windows formats with cargo-packager.
#[allow(clippy::too_many_arguments)]
async fn create_packaged_app(
    app: &App,
    config: &AppConfig,
    formats: &[AppFormat],
    platform: Platform,
    pack_file: &Path,
    launcher_source: Option<UrlOrPath>,
    staging: &Path,
    out_dir: &Path,
) -> Result<Vec<PathBuf>> {
    let launcher_config_file = staging.join(LAUNCHER_CONFIG_FILE_NAME);
    write_launcher_config(app, pack_file, &launcher_config_file)?;

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

    // Without a signing identity, macOS considers a downloaded application "damaged", as the
    // launcher is signed by the linker but the bundle is not. An ad-hoc signature (`-`) lets
    // users open it anyway in the system settings.
    let mut macos = MacOsConfig::new().signing_identity(
        config
            .macos
            .signing_identity
            .as_deref()
            .unwrap_or(AD_HOC_SIGNING_IDENTITY),
    );
    if let Some(version) = &config.macos.minimum_system_version {
        macos = macos.minimum_system_version(version);
    }
    // Lets the uninstaller remove the unpacked environment, see `LauncherConfig::app_data_dir`.
    let mut nsis = NsisConfig::new().appdata_paths(["$LOCALAPPDATA\\$IDENTIFIER"]);
    if platform.is_windows() {
        let icon = windows_icon(&app.icons, staging)?;
        brand_windows_executable(&main_binary_file, app, icon.as_deref())?;
        if let Some(icon) = icon {
            nsis = nsis.installer_icon(icon);
        }
    }
    let windows = windows_config(&config.windows);

    let resource = |src: &Path, target: &str| Resource::Mapped {
        src: src.to_string_lossy().into_owned(),
        target: PathBuf::from(target),
    };
    let mut packager_config = ConfigBuilder::new()
        .product_name(&app.name)
        .identifier(&app.identifier)
        .version(numeric_version(&app.version))
        .target_triple(target_triple(platform)?)
        .binaries([Binary::new(&main_binary).main(true)])
        .resources([
            resource(pack_file, PACK_FILE_NAME),
            resource(&launcher_config_file, LAUNCHER_CONFIG_FILE_NAME),
        ])
        .icons(app.icons.iter().map(|i| i.to_string_lossy().into_owned()))
        .formats(formats.iter().map(|f| f.package_format()))
        .out_dir(out_dir)
        .macos(macos)
        .windows(windows)
        .nsis(nsis);
    if let Some(description) = &app.description {
        packager_config = packager_config.description(description);
    }
    let packager_config = packager_config.config().clone();

    let outputs = tokio::task::spawn_blocking(move || cargo_packager::package(&packager_config))
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
    Ok(paths)
}

fn write_launcher_config(app: &App, pack_file: &Path, destination: &Path) -> Result<()> {
    let pack_sha256 = compute_file_digest::<Sha256>(pack_file)?
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let launcher_config = LauncherConfig {
        name: app.name.clone(),
        identifier: app.identifier.clone(),
        version: app.version.clone(),
        command: app.command.clone(),
        pack_file: PathBuf::from(PACK_FILE_NAME),
        pack_sha256,
    };
    std::fs::write(destination, serde_json::to_string_pretty(&launcher_config)?)?;
    Ok(())
}

fn windows_config(config: &AppWindowsConfig) -> WindowsConfig {
    let mut windows = WindowsConfig::new().tsp(config.tsp);
    if let Some(thumbprint) = &config.certificate_thumbprint {
        windows = windows.certificate_thumbprint(thumbprint);
    }
    if let Some(url) = &config.timestamp_url {
        windows = windows.timestamp_url(url);
    }
    windows.sign_command = config.sign_command.clone();
    windows
}

/// The `.ico` for Windows: a configured `.ico`, or one created from the largest `.png`.
fn windows_icon(icons: &[PathBuf], staging: &Path) -> Result<Option<PathBuf>> {
    let has_extension = |path: &&PathBuf, ext: &str| path.extension().is_some_and(|e| e == ext);
    if let Some(ico) = icons.iter().find(|i| has_extension(i, "ico")) {
        return Ok(Some(ico.clone()));
    }
    let mut largest: Option<image::DynamicImage> = None;
    for png in icons.iter().filter(|i| has_extension(i, "png")) {
        let image =
            image::open(png).with_context(|| format!("could not read icon {}", png.display()))?;
        if largest.as_ref().is_none_or(|l| image.width() > l.width()) {
            largest = Some(image);
        }
    }
    let Some(mut image) = largest else {
        return Ok(None);
    };
    // The largest size an `.ico` supports.
    if image.width() > 256 || image.height() > 256 {
        image = image.resize(256, 256, image::imageops::FilterType::Lanczos3);
    }
    let ico = staging.join("icon.ico");
    image
        .save_with_format(&ico, image::ImageFormat::Ico)
        .context("could not create .ico icon")?;
    Ok(Some(ico))
}

/// Give the launcher the application's icon, used for its shortcuts, and version info, shown e.g.
/// in the Task Manager and the file properties.
fn brand_windows_executable(executable: &Path, app: &App, icon: Option<&Path>) -> Result<()> {
    use editpe::{Image, VersionInfo, VersionStringTable, types::VersionU16};

    let contents = std::fs::read(executable)?;
    let mut image =
        Image::parse(contents.as_slice()).context("could not parse pixi-launcher.exe")?;
    let mut resources = image.resource_directory().cloned().unwrap_or_default();
    if let Some(icon) = icon {
        resources
            .set_main_icon_file(&icon.to_string_lossy())
            .context("could not set the icon of pixi-launcher.exe")?;
    }

    let file_name = format!("{}.exe", executable_name(&app.name, Platform::Win64));
    let strings = [
        ("FileDescription", app.name.as_str()),
        ("ProductName", app.name.as_str()),
        ("ProductVersion", app.version.as_str()),
        ("FileVersion", app.version.as_str()),
        ("OriginalFilename", file_name.as_str()),
        ("InternalName", file_name.as_str()),
    ];
    let version_info = VersionInfo {
        // U.S. English, Unicode.
        strings: vec![VersionStringTable {
            key: "040904B0".to_string(),
            strings: strings
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }],
        vars: vec![VersionU16 {
            major: 0x0409,
            minor: 0x04B0,
        }],
        ..Default::default()
    };
    resources
        .set_version_info(&version_info)
        .context("could not set the version info of pixi-launcher.exe")?;

    image
        .set_resource_directory(resources)
        .context("could not update the resources of pixi-launcher.exe")?;
    image
        .write_file(executable)
        .context("could not write pixi-launcher.exe")?;
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
/// Manager. cargo-packager strips everything after a `.`. On Linux, it is the name of the
/// command in the Flatpak, so it is lowercase without spaces.
fn executable_name(app_name: &str, platform: Platform) -> String {
    if platform.is_linux() {
        return app_name
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("-");
    }
    app_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '(' | ')') {
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
            vec![AppFormat::Flatpak]
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
            formats = ["app", "dmg", "nsis", "flatpak"]
            "#,
        );
        let config = config.unwrap();
        assert_eq!(
            config.formats_for(Platform::Osx64).unwrap(),
            vec![AppFormat::App, AppFormat::Dmg]
        );
        assert_eq!(
            config.formats_for(Platform::LinuxAarch64).unwrap(),
            vec![AppFormat::Flatpak]
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
    fn test_resolve_without_menu() {
        let (_dir, config) = write_config(
            r#"
            name = "Ocean Viewer"
            identifier = "org.example.ocean-viewer"
            version = "1.0.0"
            entry-point = "python"
            args = ["-m", "idlelib"]
            "#,
        );
        let app = config
            .unwrap()
            .resolve(Path::new("pack"), Platform::Linux64, Path::new("menu"))
            .unwrap();
        assert_eq!(app.command, vec!["python", "-m", "idlelib"]);
        assert_eq!(app.description, None);

        let (_dir, config) = write_config(
            r#"
            name = "Ocean Viewer"
            version = "1.0.0"
            entry-point = "python"
            "#,
        );
        let err = config
            .unwrap()
            .resolve(Path::new("pack"), Platform::Linux64, Path::new("menu"))
            .unwrap_err();
        assert!(err.to_string().contains("`identifier`"));
    }

    #[test]
    fn test_numeric_version() {
        assert_eq!(numeric_version("1.0.0"), "1.0.0");
        assert_eq!(numeric_version("1.2"), "1.2.0");
        assert_eq!(numeric_version("2024.05.1"), "2024.5.1");
        assert_eq!(numeric_version("6.0.0a1"), "6.0.0");
        assert_eq!(numeric_version("1.2.3.post1"), "1.2.3");
    }

    #[test]
    fn test_validate_identifier() {
        assert!(validate_identifier("org.example.ocean-viewer").is_ok());
        assert!(validate_identifier("x").is_err());
        assert!(validate_identifier("org.example/../x").is_err());
        assert!(validate_identifier("org..x").is_err());
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
        assert_eq!(
            executable_name("napari (0.9.2)", Platform::Linux64),
            "napari-0-9-2"
        );
    }

    #[test]
    fn test_windows_icon_from_png() {
        let dir = tempfile::tempdir().unwrap();
        let small = dir.path().join("small.png");
        let large = dir.path().join("large.png");
        image::RgbaImage::new(32, 32).save(&small).unwrap();
        image::RgbaImage::new(512, 512).save(&large).unwrap();
        let ico = windows_icon(&[small.clone(), large], dir.path())
            .unwrap()
            .unwrap();
        let icon = image::open(&ico).unwrap();
        assert_eq!((icon.width(), icon.height()), (256, 256));

        // An `.ico` is used as is.
        let configured = dir.path().join("app.ico");
        assert_eq!(
            windows_icon(&[small, configured.clone()], dir.path()).unwrap(),
            Some(configured)
        );
        assert_eq!(windows_icon(&[], dir.path()).unwrap(), None);
    }

    #[test]
    fn test_windows_signing_config() {
        let (_dir, config) = write_config(
            r#"
            name = "x"
            identifier = "x"
            version = "1"
            entry-point = "x"

            [windows]
            certificate-thumbprint = "ABC"
            timestamp-url = "http://timestamp.digicert.com"
            sign-command = "sign.sh %1"
            "#,
        );
        let windows = windows_config(&config.unwrap().windows);
        assert_eq!(windows.certificate_thumbprint.as_deref(), Some("ABC"));
        assert_eq!(
            windows.timestamp_url.as_deref(),
            Some("http://timestamp.digicert.com")
        );
        assert_eq!(windows.sign_command.as_deref(), Some("sign.sh %1"));
        assert!(!windows.tsp);
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
