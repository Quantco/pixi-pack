#![allow(clippy::too_many_arguments)]

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::{fs, io};
use std::{path::PathBuf, process::Command};
use walkdir::WalkDir;

use pixi_pack::{
    Config, DEFAULT_PIXI_PACK_VERSION, PIXI_PACK_VERSION, PackOptions, PixiPackMetadata,
    UnpackOptions, unarchive,
};
use rattler_conda_types::Platform;
use rattler_conda_types::RepoData;
use rattler_lock::UrlOrPath;
use rattler_shell::shell::{Bash, ShellEnum};
use rstest::*;
use serial_test::serial;
use tempfile::{TempDir, tempdir};
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use url::Url;

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
    #[default(Some(ShellEnum::Bash(Bash)))] shell: Option<ShellEnum>,
    #[default(false)] ignore_pypi_non_wheel: bool,
    #[default("env")] env_name: String,
    #[default(false)] create_executable: bool,
) -> Options {
    let output_dir = tempdir().expect("Couldn't create a temp dir for tests");
    let pack_file = if create_executable {
        output_dir.path().join(if platform.is_windows() {
            "environment.ps1"
        } else {
            "environment.sh"
        })
    } else {
        output_dir.path().join("environment.tar")
    };
    let metadata = PixiPackMetadata {
        version: DEFAULT_PIXI_PACK_VERSION.to_string(),
        pixi_pack_version: Some(PIXI_PACK_VERSION.to_string()),
        platform,
    };

    Options {
        pack_options: PackOptions {
            environment,
            platform,
            auth_file,
            output_file: pack_file.clone(),
            manifest_path,
            metadata,
            injected_packages: vec![],
            ignore_pypi_non_wheel,
            create_executable,
            no_tar: false,
            pixi_unpack_source: None,
            cache_dir: None,
            config: None,
        },
        unpack_options: UnpackOptions {
            pack_file,
            output_directory: output_dir.path().to_path_buf(),
            env_name,
            shell,
        },
        output_dir,
    }
}
#[fixture]
fn required_fs_objects(#[default(false)] use_pypi: bool) -> Vec<&'static str> {
    let mut required_fs_objects = vec!["conda-meta/history", "include", "share"];
    let openssl_required_file = match Platform::current() {
        Platform::Linux64 => "conda-meta/openssl-3.3.1-h4ab18f5_0.json",
        Platform::LinuxAarch64 => "conda-meta/openssl-3.3.1-h68df207_0.json",
        Platform::OsxArm64 => "conda-meta/openssl-3.3.1-hfb2fe0b_0.json",
        Platform::Osx64 => "conda-meta/openssl-3.3.1-h87427d6_0.json",
        Platform::Win64 => "conda-meta/openssl-3.3.1-h2466b09_0.json",
        _ => panic!("Unsupported platform"),
    };
    let ordered_enum_required_file = match Platform::current() {
        Platform::Linux64 => "lib/python3.11/site-packages/ordered_enum-0.0.9.dist-info",
        Platform::LinuxAarch64 => "lib/python3.11/site-packages/ordered_enum-0.0.9.dist-info",
        Platform::OsxArm64 => "lib/python3.11/site-packages/ordered_enum-0.0.9.dist-info",
        Platform::Osx64 => "lib/python3.11/site-packages/ordered_enum-0.0.9.dist-info",
        Platform::Win64 => "lib/site-packages/ordered_enum-0.0.9.dist-info",
        _ => panic!("Unsupported platform"),
    };
    if use_pypi {
        required_fs_objects.push(ordered_enum_required_file);
    } else {
        required_fs_objects.push(openssl_required_file);
    }
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
        ])
    } else {
        required_fs_objects.extend(vec!["bin/python", "lib", "man", "ssl"]);
    }
    required_fs_objects
}

#[rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn test_simple_python(
    #[case] use_pypi: bool,
    options: Options,
    #[with(use_pypi)] required_fs_objects: Vec<&'static str>,
) {
    let mut pack_options = options.pack_options;
    if use_pypi {
        pack_options.manifest_path = PathBuf::from("examples/pypi-wheel-packages/pixi.toml")
    }

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
#[case("my-webserver-0.1.0-pyh4616a5c_0.conda", true)]
#[case("my-webserver-0.1.0-pyh4616a5c_0.tar.bz2", true)]
#[case("my_webserver-0.1.0-py3-none-any.whl", false)]
#[tokio::test]
async fn test_inject(
    #[case] package_file: &str,
    #[case] is_conda: bool,
    options: Options,
    mut required_fs_objects: Vec<&'static str>,
) {
    let mut pack_options = options.pack_options;
    let unpack_options = options.unpack_options;
    let pack_file = unpack_options.pack_file.clone();

    pack_options
        .injected_packages
        .push(PathBuf::from(format!("examples/webserver/{package_file}")));

    pack_options.manifest_path = PathBuf::from("examples/webserver/pixi.toml");

    let pack_result = pixi_pack::pack(pack_options).await;
    assert!(pack_result.is_ok(), "{:?}", pack_result);
    assert!(pack_file.is_file());

    let env_dir = unpack_options.output_directory.join("env");
    let activate_file = unpack_options.output_directory.join("activate.sh");
    let unpack_result = pixi_pack::unpack(unpack_options).await;
    assert!(unpack_result.is_ok(), "{:?}", unpack_result);
    assert!(activate_file.is_file());

    // output env should contain files from the injected package
    if is_conda {
        required_fs_objects.push("conda-meta/my-webserver-0.1.0-pyh4616a5c_0.json");
    } else {
        let platform = Platform::current();
        if platform.is_windows() {
            required_fs_objects.push("lib/site-packages/my_webserver-0.1.0.dist-info");
        } else {
            required_fs_objects.push("lib/python3.12/site-packages/my_webserver-0.1.0.dist-info");
        }
    }

    required_fs_objects
        .iter()
        .map(|dir| env_dir.join(dir))
        .for_each(|dir| {
            assert!(dir.exists(), "{:?} does not exist", dir);
        });
}

#[rstest]
#[tokio::test]
async fn test_inject_failure(options: Options) {
    let mut pack_options = options.pack_options;
    pack_options.injected_packages.push(PathBuf::from(
        "examples/webserver/my-webserver-broken-0.1.0-pyh4616a5c_0.conda",
    ));
    pack_options.manifest_path = PathBuf::from("examples/webserver/pixi.toml");

    let pack_result = pixi_pack::pack(pack_options).await;

    assert!(pack_result.is_err());
    assert!(
        pack_result.err().unwrap().to_string()
            == "package 'my-webserver-broken=0.1.0=pyh4616a5c_0' has dependency 'fastapi >=0.112', which is not in the environment"
    );
}

#[rstest]
#[tokio::test]
async fn test_includes_repodata_patches(
    #[with(PathBuf::from("examples/repodata-patches/pixi.toml"))] options: Options,
) {
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
#[case("conda", false)]
#[case("micromamba", false)]
#[case("conda", true)]
#[case("micromamba", true)]
#[tokio::test]
#[serial]
async fn test_compatibility(
    #[case] tool: &str,
    #[case] use_pypi: bool,
    options: Options,
    #[with(use_pypi)] required_fs_objects: Vec<&'static str>,
) {
    let mut pack_options = options.pack_options;
    if use_pypi {
        pack_options.manifest_path = PathBuf::from("examples/pypi-wheel-packages/pixi.toml")
    }
    let pack_file = options.unpack_options.pack_file.clone();

    let pack_result = pixi_pack::pack(pack_options).await;

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
async fn test_pypi_non_wheel_ignore(
    #[with(PathBuf::from("examples/pypi-non-wheel-packages/pixi.toml"))] options: Options,
    #[case] ignore_pypi_non_wheel: bool,
    #[case] should_fail: bool,
) {
    let mut pack_options = options.pack_options;
    pack_options.ignore_pypi_non_wheel = ignore_pypi_non_wheel;
    let pack_result = pixi_pack::pack(pack_options).await;
    assert_eq!(pack_result.is_err(), should_fail);
    // Error: package pysdl2 is not a wheel file, we require all dependencies to be wheels.
    if should_fail {
        let error_message = pack_result.err().unwrap().to_string();
        assert!(
            error_message.contains("pysdl2 is not a wheel file")
                || error_message.contains("pyboy is not a wheel file"),
            "{error_message}"
        );
    }
}

fn sha256_digest_bytes(path: &PathBuf) -> String {
    let mut hasher = Sha256::new();
    let mut file = fs::File::open(path).unwrap();
    let _bytes_written = io::copy(&mut file, &mut hasher).unwrap();
    let digest = hasher.finalize();
    format!("{:X}", digest)
}

#[rstest]
#[case(Platform::Linux64, false)]
#[case(Platform::Linux64, true)]
#[case(Platform::LinuxAarch64, false)]
#[case(Platform::LinuxAarch64, true)]
#[case(Platform::LinuxPpc64le, false)]
#[case(Platform::LinuxPpc64le, true)]
#[case(Platform::OsxArm64, false)]
#[case(Platform::OsxArm64, true)]
#[case(Platform::Osx64, false)]
#[case(Platform::Osx64, true)]
#[case(Platform::Win64, false)]
#[case(Platform::Win64, true)]
// #[case(Platform::WinArm64, false)] depends on https://github.com/regro/cf-scripts/pull/3194
#[tokio::test]
async fn test_reproducible_shasum(
    #[case] platform: Platform,
    #[case] use_pypi: bool,
    #[with(PathBuf::from("examples/simple-python/pixi.toml"), "default".to_string(), platform)]
    options: Options,
) {
    let mut pack_options = options.pack_options.clone();
    if use_pypi {
        pack_options.manifest_path = PathBuf::from("examples/pypi-wheel-packages/pixi.toml")
    }
    let pack_result = pixi_pack::pack(pack_options.clone()).await;
    assert!(pack_result.is_ok(), "{:?}", pack_result);

    let sha256_digest = sha256_digest_bytes(&pack_options.output_file);
    let pypi_suffix = if use_pypi { "-pypi" } else { "" };
    insta::assert_snapshot!(
        format!("sha256-{}{}", platform, pypi_suffix),
        &sha256_digest
    );

    if platform == Platform::LinuxPpc64le {
        // pixi-pack not available for ppc64le for now
        return;
    }

    // Test with create executable
    let output_file = options.output_dir.path().join(if platform.is_windows() {
        "environment.ps1"
    } else {
        "environment.sh"
    });

    pack_options.create_executable = true;
    pack_options.output_file = output_file.clone();
    let pack_result = pixi_pack::pack(pack_options).await;
    assert!(pack_result.is_ok(), "{:?}", pack_result);

    let sha256_digest = sha256_digest_bytes(&output_file);
    insta::assert_snapshot!(
        format!("sha256-{}{}-executable", platform, pypi_suffix),
        &sha256_digest
    );
}

#[rstest]
#[case(Platform::Linux64)]
#[case(Platform::Win64)]
#[tokio::test]
async fn test_line_endings(
    #[case] platform: Platform,
    #[with(PathBuf::from("examples/simple-python/pixi.toml"), "default".to_string(), platform, None, None, false, "env".to_string(), true)]
    options: Options,
) {
    let pack_result = pixi_pack::pack(options.pack_options.clone()).await;
    assert!(pack_result.is_ok(), "{:?}", pack_result);

    let out_file = options.pack_options.output_file.clone();
    let output = fs::read_to_string(&out_file).unwrap();

    if platform.is_windows() {
        let num_crlf = output.matches("\r\n").count();
        let num_lf = output.matches("\n").count();
        assert_eq!(num_crlf, num_lf);
    } else {
        assert!(!output.contains("\r\n"));
    }
}

#[rstest]
#[tokio::test]
async fn test_non_authenticated(
    #[with(PathBuf::from("examples/auth/pixi.toml"))] options: Options,
) {
    let pack_options = options.pack_options;
    let pack_result = pixi_pack::pack(pack_options).await;
    assert!(pack_result.is_err());
    assert!(
        pack_result
            .err()
            .unwrap()
            .to_string()
            .contains("could not download package")
    );
}

#[rstest]
#[tokio::test]
async fn test_no_timestamp(
    #[with(PathBuf::from("examples/no-timestamp/pixi.toml"))] options: Options,
) {
    let mut pack_options = options.pack_options;
    pack_options.platform = Platform::Osx64;
    let pack_result = pixi_pack::pack(pack_options).await;
    assert!(pack_result.is_ok());
}

#[rstest]
#[tokio::test]
async fn test_custom_env_name(options: Options) {
    let env_name = "custom";
    let pack_options = options.pack_options;
    let pack_result = pixi_pack::pack(pack_options).await;
    assert!(pack_result.is_ok(), "{:?}", pack_result);

    let mut unpack_options = options.unpack_options;
    unpack_options.env_name = env_name.to_string();
    let env_dir = unpack_options.output_directory.join(env_name);
    let unpack_result = pixi_pack::unpack(unpack_options).await;
    assert!(unpack_result.is_ok(), "{:?}", unpack_result);
    assert!(env_dir.is_dir());
}

#[rstest]
#[tokio::test]
async fn test_run_packed_executable(options: Options, required_fs_objects: Vec<&'static str>) {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut pack_options = options.pack_options;
    pack_options.create_executable = true;

    #[cfg(target_os = "windows")]
    {
        pack_options.output_file = temp_dir.path().join("environment.ps1");
    }
    #[cfg(not(target_os = "windows"))]
    {
        pack_options.output_file = temp_dir.path().join("environment.sh");
    }

    let pack_file = pack_options.output_file.clone();

    let pack_result = pixi_pack::pack(pack_options).await;
    assert!(pack_result.is_ok(), "{:?}", pack_result);

    assert!(
        pack_file.exists(),
        "Pack file does not exist at {:?}",
        pack_file
    );

    let pack_file_contents = fs::read_to_string(&pack_file).unwrap();

    #[cfg(target_os = "windows")]
    {
        let archive_start = pack_file_contents
            .find("__END_HEADER__")
            .expect("Could not find header end marker")
            + "__END_HEADER__".len();
        let archive_end = pack_file_contents
            .find("__END_ARCHIVE__")
            .expect("Could not find archive end marker");
        let archive_bits = &pack_file_contents[archive_start..archive_end];
        assert!(!archive_bits.is_empty());

        let pixi_pack_bits = &pack_file_contents[archive_end + "__END_ARCHIVE__".len()..];
        assert!(!pixi_pack_bits.is_empty());

        assert_eq!(pack_file.extension().unwrap(), "ps1");
        let output = Command::new("powershell")
            .arg("-File")
            .arg(&pack_file)
            .arg("-o")
            .arg(options.output_dir.path())
            .output()
            .expect("Failed to execute packed file for extraction");
        assert!(
            output.status.success(),
            "Packed file execution failed: {:?}",
            output
        );
    }
    #[cfg(not(target_os = "windows"))]
    {
        assert!(pack_file_contents.contains("@@END_HEADER@@"));
        assert!(pack_file_contents.contains("@@END_ARCHIVE@@"));

        let archive_start = pack_file_contents
            .find("@@END_HEADER@@")
            .expect("Could not find header end marker")
            + "@@END_HEADER@@".len();
        let archive_end = pack_file_contents
            .find("@@END_ARCHIVE@@")
            .expect("Could not find archive end marker");
        let archive_bits = &pack_file_contents[archive_start..archive_end];
        assert!(!archive_bits.is_empty());

        let pixi_pack_bits = &pack_file_contents[archive_end + "@@END_ARCHIVE@@".len()..];
        assert!(!pixi_pack_bits.is_empty());

        assert_eq!(pack_file.extension().unwrap(), "sh");

        let output = Command::new("bash")
            .arg(&pack_file)
            .arg("-o")
            .arg(options.output_dir.path())
            .output()
            .expect("Failed to execute packed file for extraction");
        assert!(
            output.status.success(),
            "Packed file execution failed: {:?}",
            output
        );

        let output = Command::new(&pack_file)
            .arg("-o")
            .arg(options.output_dir.path())
            .output()
            .expect("Failed to execute packed file for extraction");
        assert!(
            output.status.success(),
            "Packed file execution failed: {:?}",
            output
        );
    }

    let env_dir = options
        .output_dir
        .path()
        .join(options.unpack_options.env_name);
    assert!(
        env_dir.exists(),
        "Environment directory not found after extraction"
    );

    #[cfg(target_os = "windows")]
    let activation_script = options.output_dir.path().join("activate.bat");
    #[cfg(not(target_os = "windows"))]
    let activation_script = options.output_dir.path().join("activate.sh");

    assert!(
        activation_script.exists(),
        "Activation script not found after extraction"
    );

    required_fs_objects
        .iter()
        .map(|dir| env_dir.join(dir))
        .for_each(|dir| {
            assert!(dir.exists(), "{:?} does not exist", dir);
        });

    // Keep the temporary directory alive until the end of the test
    drop(temp_dir);
}

#[rstest]
#[tokio::test]
async fn test_manifest_path_dir(#[with(PathBuf::from("examples/simple-python"))] options: Options) {
    let pack_options = options.pack_options;
    let unpack_options = options.unpack_options;
    let pack_file = unpack_options.pack_file.clone();

    let pack_result = pixi_pack::pack(pack_options).await;
    assert!(pack_result.is_ok(), "{:?}", pack_result);
    assert!(pack_file.is_file());
}

#[rstest]
#[tokio::test]
async fn test_package_caching(
    #[with(PathBuf::from("examples/simple-python/pixi.toml"))] options: Options,
) {
    let temp_cache = tempdir().expect("Couldn't create a temp cache dir");
    let cache_dir = temp_cache.path().to_path_buf();

    // First pack with cache - should download packages
    let mut pack_options = options.pack_options.clone();
    pack_options.cache_dir = Some(cache_dir.clone());
    let pack_result = pixi_pack::pack(pack_options).await;
    assert!(pack_result.is_ok(), "{:?}", pack_result);

    // Get files and their modification times after first pack
    let mut initial_cache_files = HashMap::new();
    for entry in WalkDir::new(&cache_dir) {
        let entry = entry.unwrap();
        if entry.file_type().is_file() {
            let path = entry.path().to_path_buf();
            let modified_time = fs::metadata(&path).unwrap().modified().unwrap();
            initial_cache_files.insert(path, modified_time);
        }
    }
    assert!(
        !initial_cache_files.is_empty(),
        "Cache should contain downloaded files"
    );

    // Calculate first pack's SHA256, reusing test_reproducible_shasum
    let first_sha256 = sha256_digest_bytes(&options.pack_options.output_file);
    insta::assert_snapshot!(
        format!("sha256-{}", options.pack_options.platform),
        &first_sha256
    );

    // Small delay to ensure any new writes would have different timestamps
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    // Second pack with same cache - should use cached packages
    let temp_dir2 = tempdir().expect("Couldn't create second temp dir");
    let mut pack_options2 = options.pack_options.clone();
    pack_options2.cache_dir = Some(cache_dir.clone());
    let output_file2 = temp_dir2.path().join("environment.tar");
    pack_options2.output_file = output_file2.clone();

    let pack_result2 = pixi_pack::pack(pack_options2).await;
    assert!(pack_result2.is_ok(), "{:?}", pack_result2);

    // Check that cache files weren't modified
    for (path, initial_mtime) in initial_cache_files {
        let current_mtime = fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(
            initial_mtime,
            current_mtime,
            "Cache file {} was modified when it should have been reused",
            path.display()
        );
    }

    // Verify second pack produces identical output
    let second_sha256 = sha256_digest_bytes(&output_file2);
    assert_eq!(
        first_sha256, second_sha256,
        "Pack outputs should be identical when using cache"
    );

    // Both output files should exist and be valid
    assert!(options.pack_options.output_file.exists());
    assert!(output_file2.exists());
}

#[rstest]
#[tokio::test]
async fn test_mirror_middleware(
    #[with(PathBuf::from("examples/mirror-middleware/pixi.toml"))] options: Options,
) {
    let mut pack_options = options.pack_options;
    pack_options.config = Some(
        Config::load_from_files(vec![&PathBuf::from(
            "examples/mirror-middleware/config.toml",
        )])
        .unwrap(),
    );
    let pack_result = pixi_pack::pack(pack_options).await;
    assert!(pack_result.is_ok(), "{:?}", pack_result);
}

#[rstest]
#[tokio::test]
async fn test_pixi_pack_source(
    #[with(PathBuf::from("examples/simple-python/pixi.toml"), "default".to_string(), Platform::Linux64)]
    options: Options,
) {
    let platform = Platform::Linux64;
    let temp_dir = tempfile::tempdir().unwrap();
    let mut pack_options = options.pack_options.clone();
    let output_file = options.output_dir.path().join("environment.sh");

    pack_options.create_executable = true;
    pack_options.output_file = output_file.clone();

    // Build the path
    let version = env!("CARGO_PKG_VERSION");
    let pixi_pack_url = format!(
        "https://github.com/Quantco/pixi-pack/releases/download/v{}/pixi-unpack-x86_64-unknown-linux-musl",
        version
    );
    // Download the pixi-pack binary from the specified URL
    let pixi_unpack_path = temp_dir
        .path()
        .join("pixi-unpack-x86_64-unknown-linux-musl");
    let response = reqwest::get(&pixi_pack_url)
        .await
        .expect("Failed to download pixi-pack binary");
    let mut file = fs::File::create(&pixi_unpack_path).unwrap();
    let content = response.bytes().await.unwrap();
    io::copy(&mut content.as_ref(), &mut file).unwrap();

    // Reference the local path
    pack_options.pixi_unpack_source = Some(UrlOrPath::Path(
        pixi_unpack_path
            .into_os_string()
            .into_string()
            .unwrap()
            .into(),
    ));

    let pack_result = pixi_pack::pack(pack_options.clone()).await;
    assert!(pack_result.is_ok(), "{:?}", pack_result);

    let sha256_digest = sha256_digest_bytes(&output_file);
    insta::assert_snapshot!(format!("sha256-{}-executable", platform), &sha256_digest);

    // Keep the temporary directory alive until the end of the first test
    drop(temp_dir);

    // Now test with URL
    pack_options.pixi_unpack_source = Some(UrlOrPath::Url(Url::parse(&pixi_pack_url).unwrap()));

    let pack_result = pixi_pack::pack(pack_options.clone()).await;
    assert!(pack_result.is_ok(), "{:?}", pack_result);

    let sha256_digest = sha256_digest_bytes(&output_file);
    insta::assert_snapshot!(format!("sha256-{}-executable", platform), &sha256_digest);
}

#[fixture]
fn templated_pixi_toml() -> (PathBuf, TempDir) {
    use url::Url;
    let temp_pixi_project = tempdir().expect("Couldn't create a temp dir for tests");
    let absolute_path_to_local_channel =
        std::path::absolute("examples/local-channel/channel").unwrap();
    let absolute_path_to_package = absolute_path_to_local_channel
        .join("noarch/my-webserver-0.1.0-pyh4616a5c_0.conda")
        .to_str()
        .unwrap()
        .to_owned();
    let local_channel_url = Url::from_directory_path(&absolute_path_to_local_channel)
        .unwrap()
        .to_string();

    let pixi_toml = temp_pixi_project.path().join("pixi.toml");
    let pixi_lock = temp_pixi_project.path().join("pixi.lock");
    let pixi_toml_contents = fs::read_to_string("examples/local-channel/pixi.toml.template")
        .unwrap()
        .replace("<local-channel-url>", &local_channel_url);
    let pixi_lock_contents = fs::read_to_string("examples/local-channel/pixi.lock.template")
        .unwrap()
        .replace("<local-channel-url>", &local_channel_url)
        .replace("<absolute-path-to-package>", &absolute_path_to_package);

    fs::write(&pixi_toml, pixi_toml_contents).unwrap();
    fs::write(&pixi_lock, pixi_lock_contents).unwrap();

    (pixi_toml, temp_pixi_project)
}

#[rstest]
#[tokio::test]
async fn test_local_channel(
    #[allow(unused_variables)] templated_pixi_toml: (PathBuf, TempDir),
    #[with(templated_pixi_toml.0.clone())] options: Options,
) {
    let pack_options = options.pack_options;
    let pack_result = pixi_pack::pack(pack_options).await;
    assert!(pack_result.is_ok(), "{:?}", pack_result);

    let unpack_options = options.unpack_options;
    let unpack_result = pixi_pack::unpack(unpack_options.clone()).await;
    assert!(unpack_result.is_ok(), "{:?}", unpack_result);

    assert!(
        unpack_options
            .output_directory
            .join("env")
            .join("conda-meta")
            .join("my-webserver-0.1.0-pyh4616a5c_0.json")
            .exists()
    );
}
