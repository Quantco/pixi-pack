//! Reading application details from a package's menuinst shortcut definition (`Menu/*.json`).
//! <https://conda.github.io/menuinst/>

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use rattler_conda_types::{Platform, RepoData};
use rattler_menuinst::schema::{MenuInstSchema, MenuItemCommand, NameField};

use crate::{CHANNEL_DIRECTORY_NAME, menu_placeholders, replace_placeholders};

/// Desktop entry field codes for the files or URLs to open.
const FIELD_CODES: [&str; 4] = ["%F", "%f", "%U", "%u"];
/// Shortcuts pass their arguments on with these, the launcher does so itself.
const ARGUMENT_PLACEHOLDERS: [&str; 5] = ["%*", "%F", "%f", "%U", "%u"];

/// A menu item of a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MenuApp {
    pub name: String,
    pub description: String,
    /// The command with menuinst placeholders, resolved when the environment is installed.
    pub command: Vec<String>,
    pub icon: Option<PathBuf>,
    /// `CFBundleIdentifier` of the macOS shortcut.
    pub identifier: Option<String>,
    pub version: String,
    /// `Categories` of the Linux shortcut.
    pub categories: Vec<String>,
    /// `StartupWMClass` of the Linux shortcut.
    pub startup_wm_class: Option<String>,
    /// `MimeType` of the Linux shortcut.
    pub mime_types: Vec<String>,
    /// Field code like `%F` of the Linux shortcut's command, for the files to open.
    pub field_code: Option<String>,
}

/// Read the menu item of `package` for `platform` from the pack in `pack_dir`.
/// `item` selects an item by name if the package defines several.
/// The package is extracted into `extract_dir`, which has to outlive the returned icon path.
pub(super) fn load(
    pack_dir: &Path,
    platform: Platform,
    package: &str,
    item: Option<&str>,
    extract_dir: &Path,
) -> Result<MenuApp> {
    let (archive, version) = find_package(pack_dir, platform, package)?;
    rattler_package_streaming::fs::extract(&archive, extract_dir)
        .with_context(|| format!("could not extract {}", archive.display()))?;

    let mut items = Vec::new();
    let menu_dir = extract_dir.join("Menu");
    for entry in std::fs::read_dir(&menu_dir)
        .with_context(|| format!("package `{package}` has no menuinst shortcuts in Menu/"))?
    {
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let contents = std::fs::read_to_string(&path)?;
        let schema: MenuInstSchema = serde_json::from_str(&contents)
            .with_context(|| format!("could not parse {}", path.display()))?;
        items.extend(schema.menu_items);
    }

    let mut apps = items
        .into_iter()
        .map(|i| menu_app(i, platform, extract_dir, &version))
        .collect::<Vec<_>>();
    let names = apps.iter().map(|a| a.name.clone()).collect::<Vec<_>>();
    let index = match (item, apps.len()) {
        (_, 0) => bail!("package `{package}` has no menuinst shortcuts in Menu/"),
        (None, 1) => 0,
        (None, _) => bail!(
            "package `{package}` has several menu items, select one with `menu-item`: {names:?}"
        ),
        (Some(item), _) => names.iter().position(|n| n == item).ok_or_else(|| {
            anyhow!("package `{package}` has no menu item `{item}`, available: {names:?}")
        })?,
    };
    Ok(apps.swap_remove(index))
}

/// The archive of `package` in the pack and its version.
fn find_package(pack_dir: &Path, platform: Platform, package: &str) -> Result<(PathBuf, String)> {
    for subdir in [platform, Platform::NoArch] {
        let subdir_dir = pack_dir.join(CHANNEL_DIRECTORY_NAME).join(subdir.as_str());
        let repodata_path = subdir_dir.join("repodata.json");
        if !repodata_path.exists() {
            continue;
        }
        let repodata = RepoData::from_path(&repodata_path)
            .with_context(|| format!("could not read {}", repodata_path.display()))?;
        let found = repodata
            .packages
            .iter()
            .chain(repodata.conda_packages.iter())
            .find(|(_, record)| record.name.as_normalized() == package);
        if let Some((identifier, record)) = found {
            return Ok((
                subdir_dir.join(identifier.to_file_name()),
                record.version.to_string(),
            ));
        }
    }
    bail!("package `{package}` given as `menu` is not in the environment")
}

fn menu_app(
    item: rattler_menuinst::schema::MenuItem,
    platform: Platform,
    package_dir: &Path,
    version: &str,
) -> MenuApp {
    let platforms = item.platforms;
    let osx = platforms.osx.as_ref().map(|p| &p.specific);
    let linux = platforms.linux.as_ref().map(|p| &p.specific);
    let base = if platform.is_osx() {
        platforms.osx.as_ref().map(|p| p.base.clone())
    } else if platform.is_windows() {
        platforms.win.as_ref().map(|p| p.base.clone())
    } else {
        platforms.linux.as_ref().map(|p| p.base.clone())
    };
    let command: MenuItemCommand = match base {
        Some(base) => item.command.merge(base),
        None => item.command,
    };

    // Icons are resolved now, against the extracted package.
    let placeholders = menu_placeholders(package_dir, platform);
    let resolve = |s: &str| replace_placeholders(s, &placeholders);

    let mut name = match &command.name {
        NameField::Simple(name) => name.0.clone(),
        NameField::Complex(names) => names.target_environment_is_base.0.clone(),
    };
    // E.g. napari's name includes its version, the macOS bundle name does not.
    if platform.is_osx()
        && let Some(osx_name) = osx.and_then(|o| {
            o.cf_bundle_display_name
                .as_ref()
                .or(o.cf_bundle_name.as_ref())
        })
    {
        name = osx_name.0.clone();
    }

    let field_code = command
        .command
        .iter()
        .find(|c| FIELD_CODES.contains(&c.0.as_str()))
        .map(|c| c.0.clone());
    let mut command_args: Vec<String> = command
        .command
        .iter()
        .map(|c| c.0.clone())
        .filter(|c| !ARGUMENT_PLACEHOLDERS.contains(&c.as_str()))
        .collect();
    // macOS shortcuts start e.g. python from a link inside the shortcut's bundle, so the Dock
    // shows the shortcut. The launcher starts it from the environment and takes care of that.
    if platform.is_osx()
        && let Some(links) = osx.and_then(|o| o.link_in_bundle.as_ref())
    {
        for arg in &mut command_args {
            if let Some((source, _)) = links.iter().find(|(_, target)| target.0 == *arg) {
                *arg = source.0.clone();
            }
        }
    }

    MenuApp {
        name,
        description: command.description.0.clone(),
        command: command_args,
        icon: command
            .icon
            .as_ref()
            .map(|i| PathBuf::from(resolve(&i.0)))
            .filter(|i| i.is_file()),
        identifier: osx
            .and_then(|o| o.cf_bundle_identifier.as_ref())
            .map(|i| i.0.clone()),
        version: version.to_string(),
        categories: linux
            .and_then(|l| l.categories.as_ref())
            .map(|c| c.iter().map(|c| c.0.clone()).collect())
            .unwrap_or_default(),
        startup_wm_class: linux
            .and_then(|l| l.startup_wm_class.as_ref())
            .map(|c| c.0.clone()),
        mime_types: linux
            .and_then(|l| l.mime_type.as_ref())
            .map(|m| m.iter().map(|m| m.0.clone()).collect())
            .unwrap_or_default(),
        field_code,
    }
}

/* --------------------------------------------------------------------------------------------- */
/*                                             TESTS                                             */
/* --------------------------------------------------------------------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;

    // Shortened from napari's shortcut.
    const NAPARI_MENU: &str = r#"{
      "$schema": "https://json-schema.org/draft-07/schema",
      "$id": "https://schemas.conda.io/menuinst-1.schema.json",
      "menu_name": "napari (0.5.6)",
      "menu_items": [
        {
          "name": {
            "target_environment_is_base": "napari (0.5.6)",
            "target_environment_is_not_base": "napari (0.5.6) ({{ ENV_NAME }})"
          },
          "description": "a fast n-dimensional image viewer in Python",
          "icon": "{{ MENU_DIR }}/napari.{{ ICON_EXT }}",
          "precommand": "unset PYTHONHOME && unset PYTHONPATH",
          "command": ["{{ PYTHON }}", "-m", "napari"],
          "activate": true,
          "terminal": false,
          "platforms": {
            "win": {
              "precommand": "set \"PYTHONHOME=\" & set \"PYTHONPATH=\"",
              "command": ["{{ PYTHONW }}", "-m", "napari", "%*"],
              "desktop": true
            },
            "linux": {
              "Categories": ["Graphics", "Science"],
              "command": ["{{ PYTHON }}", "-m", "napari", "%F"],
              "StartupWMClass": "napari",
              "MimeType": ["image/png"]
            },
            "osx": {
              "link_in_bundle": {
                "{{ PREFIX }}/bin/python": "{{ MENU_ITEM_LOCATION }}/Contents/Resources/python"
              },
              "command": ["{{ MENU_ITEM_LOCATION }}/Contents/Resources/python", "-m", "napari"],
              "CFBundleName": "napari",
              "CFBundleDisplayName": "napari",
              "CFBundleIdentifier": "org.napari.napari",
              "CFBundleVersion": "0.5.6"
            }
          }
        }
      ]
    }"#;

    fn napari_item() -> rattler_menuinst::schema::MenuItem {
        let schema: MenuInstSchema = serde_json::from_str(NAPARI_MENU).unwrap();
        schema.menu_items.into_iter().next().unwrap()
    }

    #[test]
    fn test_menu_app_for_platforms() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("Menu")).unwrap();
        std::fs::write(dir.path().join("Menu/napari.png"), "").unwrap();

        let linux = menu_app(napari_item(), Platform::Linux64, dir.path(), "0.5.6");
        assert_eq!(
            linux,
            MenuApp {
                name: "napari (0.5.6)".to_string(),
                description: "a fast n-dimensional image viewer in Python".to_string(),
                command: vec!["{{ PYTHON }}".into(), "-m".into(), "napari".into()],
                icon: Some(dir.path().join("Menu/napari.png")),
                identifier: Some("org.napari.napari".to_string()),
                version: "0.5.6".to_string(),
                categories: vec!["Graphics".to_string(), "Science".to_string()],
                startup_wm_class: Some("napari".to_string()),
                mime_types: vec!["image/png".to_string()],
                field_code: Some("%F".to_string()),
            }
        );

        let windows = menu_app(napari_item(), Platform::Win64, dir.path(), "0.5.6");
        assert_eq!(windows.command, vec!["{{ PYTHONW }}", "-m", "napari"]);
        assert_eq!(windows.field_code, None);
        // There is no `.ico` in the package.
        assert_eq!(windows.icon, None);

        let macos = menu_app(napari_item(), Platform::OsxArm64, dir.path(), "0.5.6");
        assert_eq!(macos.name, "napari");
        assert_eq!(
            macos.command,
            vec!["{{ PREFIX }}/bin/python", "-m", "napari"]
        );
    }

    #[test]
    fn test_load_menu_from_pack() {
        let pack = tempfile::tempdir().unwrap();
        let noarch = pack.path().join(CHANNEL_DIRECTORY_NAME).join("noarch");
        std::fs::create_dir_all(&noarch).unwrap();

        // A minimal `.tar.bz2` package with a shortcut.
        let package = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(package.path().join("Menu")).unwrap();
        std::fs::write(package.path().join("Menu/napari.json"), NAPARI_MENU).unwrap();
        let archive = noarch.join("napari-0.5.6-pyh_0.tar.bz2");
        rattler_package_streaming::write::write_tar_bz2_package(
            std::fs::File::create(&archive).unwrap(),
            package.path(),
            &[package.path().join("Menu/napari.json")],
            rattler_conda_types::compression_level::CompressionLevel::Default,
            None,
            None,
        )
        .unwrap();
        std::fs::write(
            noarch.join("repodata.json"),
            r#"{
              "info": {"subdir": "noarch"},
              "packages": {
                "napari-0.5.6-pyh_0.tar.bz2": {
                  "name": "napari", "version": "0.5.6", "build": "pyh_0", "build_number": 0,
                  "depends": [], "subdir": "noarch"
                }
              },
              "packages.conda": {}
            }"#,
        )
        .unwrap();

        let extract_dir = tempfile::tempdir().unwrap();
        let app = load(
            pack.path(),
            Platform::Linux64,
            "napari",
            None,
            extract_dir.path(),
        )
        .unwrap();
        assert_eq!(app.version, "0.5.6");
        assert_eq!(app.command, vec!["{{ PYTHON }}", "-m", "napari"]);

        assert!(
            load(
                pack.path(),
                Platform::Linux64,
                "napari",
                Some("other"),
                extract_dir.path()
            )
            .is_err()
        );
        assert!(
            load(
                pack.path(),
                Platform::Linux64,
                "numpy",
                None,
                extract_dir.path()
            )
            .is_err()
        );
    }
}
