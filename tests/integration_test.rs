#![allow(clippy::too_many_arguments)]

use std::{path::PathBuf, process::Command};

use async_compression::Level;
use pixi_pack::{unarchive, PackOptions, PixiPackMetadata, UnpackOptions};
use rattler_conda_types::Platform;
use rattler_shell::shell::{Bash, ShellEnum};
use rstest::*;
use tempfile::{tempdir, TempDir};

struct Options {
    pack_options: PackOptions,
    unpack_options: UnpackOptions,
    #[allow(dead_code)] // needed, otherwise output_dir is not created
    output_dir: TempDir,
}

#[fixture]
fn options(
    #[default("default")] environment: String,
    #[default(Platform::current())] platform: Platform,
    #[default(None)] auth_file: Option<PathBuf>,
    #[default(PathBuf::from("examples/simple-python/pixi.toml"))] manifest_path: PathBuf,
    #[default(PixiPackMetadata::default())] metadata: PixiPackMetadata,
    #[default(Some(Level::Best))] level: Option<Level>,
    #[default(Some(ShellEnum::Bash(Bash)))] shell: Option<ShellEnum>,
    #[default(true)] ignore_pypi_errors: bool,
) -> Options {
    let output_dir = tempdir().expect("Couldn't create a temp dir for tests");
    let pack_file = output_dir.path().join("environment.tar.zstd");
    Options {
        pack_options: PackOptions {
            environment,
            platform,
            auth_file,
            output_file: pack_file.clone(),
            manifest_path,
            metadata,
            level,
            ignore_pypi_errors,
        },
        unpack_options: UnpackOptions {
            pack_file,
            output_directory: output_dir.path().to_path_buf(),
            shell,
        },
        output_dir,
    }
}

#[fixture]
fn required_fs_objects() -> Vec<&'static str> {
    let mut required_fs_objects = vec!["conda-meta/history", "include", "share"];
    if cfg!(windows) {
        required_fs_objects.extend(vec![
            "DLLs",
            "etc",
            "Lib",
            "Library",
            "libs",
            "Scripts",
            "Tools",
            "python.exe",
            "conda-meta/openssl-3.3.0-h2466b09_3.json",
        ])
    } else {
        required_fs_objects.extend(vec!["bin/python", "lib", "man", "ssl"]);
        if cfg!(target_os = "macos") {
            // osx-arm64
            required_fs_objects.push("conda-meta/openssl-3.3.0-hfb2fe0b_3.json");
        } else {
            // linux-64
            required_fs_objects.push("conda-meta/openssl-3.3.0-h4ab18f5_3.json");
        }
    }
    required_fs_objects
}

#[rstest]
#[tokio::test]
async fn test_simple_python(options: Options, required_fs_objects: Vec<&'static str>) {
    let pack_options = options.pack_options;
    let unpack_options = options.unpack_options;
    let pack_file = unpack_options.pack_file.clone();

    let pack_result = pixi_pack::pack(pack_options).await;
    assert!(pack_result.is_ok());
    assert!(pack_file.is_file());
    assert!(pack_file.exists());

    let env_dir = unpack_options.output_directory.join("env");
    let activate_file = unpack_options.output_directory.join("activate.sh");
    let unpack_result = pixi_pack::unpack(unpack_options).await;
    assert!(unpack_result.is_ok());
    assert!(activate_file.is_file());
    assert!(activate_file.exists());

    required_fs_objects
        .iter()
        .map(|dir| env_dir.join(dir))
        .for_each(|dir| {
            assert!(dir.exists(), "{:?} does not exist", dir);
        });
}

#[cfg(not(target_os = "windows"))] // https://github.com/Quantco/pixi-pack/issues/8
#[rstest]
#[case("conda")]
#[case("micromamba")]
#[tokio::test]
async fn test_compatibility(
    #[case] tool: &str,
    options: Options,
    required_fs_objects: Vec<&'static str>,
) {
    let pack_options = options.pack_options;
    let pack_file = options.unpack_options.pack_file.clone();

    let pack_result = pixi_pack::pack(pack_options).await;
    println!("{:?}", pack_result);
    assert!(pack_result.is_ok());
    assert!(pack_file.is_file());
    assert!(pack_file.exists());

    let unpack_dir = tempdir().expect("Couldn't create a temp dir for tests");
    let unpack_dir = unpack_dir.path();
    unarchive(pack_file.as_path(), unpack_dir)
        .await
        .expect("Failed to unarchive environment");
    let environment_file = unpack_dir.join("environment.yml");
    let channel = unpack_dir.join("channel");
    assert!(environment_file.is_file());
    assert!(environment_file.exists());
    assert!(channel.is_dir());
    assert!(channel.exists());

    let create_prefix = tempdir().expect("Couldn't create a temp dir for tests");
    let create_prefix = create_prefix.path().join(tool);
    let prefix_str = create_prefix
        .to_str()
        .expect("Couldn't create conda prefix string");
    let args = if tool == "conda" {
        vec![
            "env",
            "create",
            "-y",
            "-p",
            prefix_str,
            "-f",
            "environment.yml",
        ]
    } else {
        vec!["create", "-y", "-p", prefix_str, "-f", "environment.yml"]
    };
    let output = Command::new(tool)
        .args(args)
        .current_dir(unpack_dir)
        .output()
        .expect("Failed to run create command");
    assert!(
        output.status.success(),
        "Failed to create environment: {:?}",
        output
    );

    required_fs_objects
        .iter()
        .map(|dir| create_prefix.join(dir))
        .for_each(|dir| {
            assert!(dir.exists(), "{:?} does not exist", dir);
        });
}
