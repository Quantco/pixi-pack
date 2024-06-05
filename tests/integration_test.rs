#![allow(clippy::too_many_arguments)]

use std::{path::PathBuf, process::Command};

use async_compression::Level;
use pixi_pack::{unarchive, PackOptions, PixiPackMetadata, UnpackOptions};
use rattler_conda_types::Platform;
use rattler_conda_types::RepoData;
use rattler_shell::shell::{Bash, ShellEnum};
use rstest::*;
use tempfile::{tempdir, TempDir};
use tokio::fs::File;
use tokio::io::AsyncReadExt;

struct Options {
    pack_options: PackOptions,
    unpack_options: UnpackOptions,
    #[allow(dead_code)] // needed, otherwise output_dir is not created
    output_dir: TempDir,
}

#[fixture]
fn options(
    #[default(PathBuf::from("examples/simple-python/pixi.toml"))] manifest_path: PathBuf,
    #[default("default")] environment: String,
    #[default(Platform::current())] platform: Platform,
    #[default(None)] auth_file: Option<PathBuf>,
    #[default(PixiPackMetadata::default())] metadata: PixiPackMetadata,
    #[default(Some(Level::Best))] level: Option<Level>,
    #[default(Some(ShellEnum::Bash(Bash)))] shell: Option<ShellEnum>,
    #[default(false)] ignore_pypi_errors: bool,
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
            injected_packages: vec![],
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
    let openssl_required_file = match Platform::current() {
        Platform::Linux64 => "conda-meta/openssl-3.3.0-h4ab18f5_3.json",
        Platform::LinuxAarch64 => "conda-meta/openssl-3.3.1-h68df207_0.json",
        Platform::OsxArm64 => "conda-meta/openssl-3.3.0-hfb2fe0b_3.json",
        Platform::Osx64 => "conda-meta/openssl-3.3.1-h87427d6_0.json",
        Platform::Win64 => "conda-meta/openssl-3.3.0-h2466b09_3.json",
        _ => panic!("Unsupported platform"),
    };
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
            openssl_required_file,
        ])
    } else {
        required_fs_objects.extend(vec![
            "bin/python",
            "lib",
            "man",
            "ssl",
            openssl_required_file,
        ]);
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
    assert!(pack_result.is_ok(), "{:?}", pack_result);
    assert!(pack_file.is_file());

    let env_dir = unpack_options.output_directory.join("env");
    let activate_file = unpack_options.output_directory.join("activate.sh");
    let unpack_result = pixi_pack::unpack(unpack_options).await;
    assert!(unpack_result.is_ok(), "{:?}", unpack_result);
    assert!(activate_file.is_file());

    required_fs_objects
        .iter()
        .map(|dir| env_dir.join(dir))
        .for_each(|dir| {
            assert!(dir.exists(), "{:?} does not exist", dir);
        });
}

#[rstest]
#[case("conda")]
#[case("tar.bz2")]
#[tokio::test]
async fn test_inject(
    #[case] package_format: &str,
    options: Options,
    mut required_fs_objects: Vec<&'static str>,
) {
    let mut pack_options = options.pack_options;
    let unpack_options = options.unpack_options;
    let pack_file = unpack_options.pack_file.clone();

    pack_options.injected_packages.push(PathBuf::from(format!(
        "examples/webserver/my-webserver-0.1.0-pyh4616a5c_0.{package_format}"
    )));

    pack_options.manifest_path = PathBuf::from("examples/webserver/pixi.toml");

    let pack_result = pixi_pack::pack(pack_options).await;
    assert!(pack_result.is_ok());
    assert!(pack_file.is_file());

    let env_dir = unpack_options.output_directory.join("env");
    let activate_file = unpack_options.output_directory.join("activate.sh");
    let unpack_result = pixi_pack::unpack(unpack_options).await;
    assert!(unpack_result.is_ok());
    assert!(activate_file.is_file());

    // output env should contain files from the injected package
    required_fs_objects.push("conda-meta/my-webserver-0.1.0-pyh4616a5c_0.json");

    required_fs_objects
        .iter()
        .map(|dir| env_dir.join(dir))
        .for_each(|dir| {
            assert!(dir.exists(), "{:?} does not exist", dir);
        });
}

#[rstest]
#[tokio::test]
async fn test_includes_repodata_patches(options: Options) {
    let mut pack_options = options.pack_options;
    pack_options.platform = Platform::Win64;
    let pack_file = options.unpack_options.pack_file.clone();

    let pack_result = pixi_pack::pack(pack_options).await;
    assert!(pack_result.is_ok());

    let unpack_dir = tempdir().expect("Couldn't create a temp dir for tests");
    let unpack_dir = unpack_dir.path();
    unarchive(pack_file.as_path(), unpack_dir)
        .await
        .expect("Failed to unarchive environment");

    let mut repodata_raw = String::new();

    File::open(unpack_dir.join("channel/win-64/repodata.json"))
        .await
        .expect("Failed to open repodata")
        .read_to_string(&mut repodata_raw)
        .await
        .expect("could not read repodata.json");

    let repodata: RepoData = serde_json::from_str(&repodata_raw).expect("cant parse repodata.json");

    // in this example, the `libzlib` entry in the `python-3.12.3-h2628c8c_0_cpython.conda`
    // package is `libzlib >=1.2.13,<1.3.0a0`, but the upstream repodata was patched to
    // `libzlib >=1.2.13,<2.0.0a0` which is represented in the `pixi.lock` file
    assert!(
        repodata
            .conda_packages
            .get("python-3.12.3-h2628c8c_0_cpython.conda")
            .expect("python not found in repodata")
            .depends
            .contains(&"libzlib >=1.2.13,<2.0.0a0".to_string()),
        "'libzlib >=1.2.13,<2.0.0a0' not found in python dependencies"
    );
}

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
    assert!(pack_result.is_ok(), "{:?}", pack_result);
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
    let args = vec![
        "env",
        "create",
        "-y",
        "-p",
        prefix_str,
        "-f",
        "environment.yml",
    ];
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

#[rstest]
#[case(true, false)]
#[case(false, true)]
#[tokio::test]
async fn test_pypi_ignore(
    #[with(PathBuf::from("examples/pypi-packages/pixi.toml"))] options: Options,
    #[case] ignore_pypi_errors: bool,
    #[case] should_fail: bool,
) {
    let mut pack_options = options.pack_options;
    pack_options.ignore_pypi_errors = ignore_pypi_errors;
    let pack_result = pixi_pack::pack(pack_options).await;
    assert_eq!(pack_result.is_err(), should_fail);
}
