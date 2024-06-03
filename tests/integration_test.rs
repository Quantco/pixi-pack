use std::path::PathBuf;

use async_compression::Level;
use pixi_pack::{PackOptions, PixiPackMetadata, UnpackOptions};
use rattler_conda_types::Platform;
use rattler_shell::shell::{Bash, ShellEnum};
use rstest::*;
use tempfile::{tempdir, TempDir};

struct Options {
    pack_options: PackOptions,
    unpack_options: UnpackOptions,
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
        },
        unpack_options: UnpackOptions {
            pack_file,
            output_directory: output_dir.path().to_path_buf(),
            shell,
        },
        output_dir,
    }
}

#[rstest]
#[tokio::test]
async fn test_simple_python(options: Options) {
    let pack_options = options.pack_options;
    let unpack_options = options.unpack_options;
    let _output_dir = options.output_dir;
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
