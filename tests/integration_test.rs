use std::path::PathBuf;

use async_compression::Level;
use pixi_pack::{PackOptions, PixiPackMetadata, UnpackOptions};
use rattler_conda_types::Platform;
use rattler_shell::shell::{Bash, ShellEnum};
use rstest::*;
use tempfile::tempdir;

struct Options(PackOptions, UnpackOptions);

#[fixture]
fn options(
    #[default("default")] environment: String,
    #[default(Platform::current())] platform: Platform,
    #[default(None)] auth_file: Option<PathBuf>,
    #[default(PathBuf::from("examples/simple-python/pixi.toml"))] manifest_path: PathBuf,
    #[default(PixiPackMetadata::default())] metadata: PixiPackMetadata,
    #[default(Some(Level::Best))] level: Option<Level>,
    #[default(Some(ShellEnum::Bash(Bash)))] shell: Option<ShellEnum>,
) -> Options {
    let output_dir = tempdir().expect("Couldn't create a temp dir for tests");
    let pack_file = output_dir.path().join("environment.tar.zstd");
    Options(
        PackOptions {
            environment,
            platform,
            auth_file,
            output_file: pack_file.clone(),
            manifest_path,
            metadata,
            level,
        },
        UnpackOptions {
            pack_file: pack_file,
            output_directory: output_dir.path().to_path_buf(),
            shell: shell,
        },
    )
}

#[rstest]
#[tokio::test]
async fn test_simple_python(options: Options) {
    let mut pack_options = options.0;
    let mut unpack_options = options.1;
    let temp_dir = tempdir().expect("Couldn't create a temp dir for tests");
    let pack_file = temp_dir.path().join("environment.tar.zstd");
    pack_options.output_file = pack_file.clone();
    unpack_options.pack_file = pack_file.clone();
    unpack_options.output_directory = temp_dir.path().to_path_buf();

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
    let required_fs_objects = if cfg!(windows) {
        vec![
            "conda-meta",
            "DLLs",
            "etc",
            "include",
            "Lib",
            "Library",
            "libs",
            "Scripts",
            "share",
            "Tools",
            "python.exe",
        ]
    } else {
        vec![
            "bin/python",
            "conda-meta",
            "include",
            "lib",
            "man",
            "share",
            "ssl",
        ]
    };
    required_fs_objects
        .iter()
        .map(|dir| env_dir.join(dir))
        .for_each(|dir| {
            assert!(dir.exists(), "{:?} does not exist", dir);
        });
}
