use std::path::{Path, PathBuf};

use rattler_package_streaming::fs::{extract_conda, extract_tar_bz2};

/* ------------------------------------------- UNPACK ------------------------------------------ */

/// Options for unpacking a pixi environment.
pub struct UnpackOptions {
    pub pack_file: PathBuf,
    pub target_dir: PathBuf,
}

/// Unpack a pixi environment.
pub async fn unpack(options: UnpackOptions) -> Result<(), Box<dyn std::error::Error>> {
    let unpack_dir = options.target_dir.join("unpack");
    std::fs::create_dir_all(&unpack_dir).expect("Could not create unpack directory");
    unarchive(&options.pack_file, &unpack_dir);

    // TODO: Parallelize unpacking.
    let packages = collect_packages(&unpack_dir).unwrap();
    let _ = packages.into_iter().map(|package| {
        let file_extension = package.extension().unwrap();
        let results = match file_extension.to_str().unwrap() {
            "bz2" => extract_tar_bz2(package.as_path(), &options.target_dir),
            "conda" => extract_conda(package.as_path(), &options.target_dir),
            _ => panic!("Unsupported file extension"),
        };
        results
    });

    std::fs::remove_dir_all(unpack_dir).expect("Could not remove unpack directory");

    Ok(())
}

/* -------------------------------------- INSTALL PACKAGES ------------------------------------- */

/// Collect all packages in a directory.
fn collect_packages(channel: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let subdirs = channel.read_dir()?;
    let packages = subdirs
        .into_iter()
        .flat_map(|subdir| {
            let subdir = subdir.unwrap().path();
            let packages = subdir.read_dir().unwrap();
            packages
                .into_iter()
                .map(|package| package.unwrap().path())
                .collect::<Vec<PathBuf>>()
        })
        .collect();
    Ok(packages)
}

/* ----------------------------------- UNARCHIVE + DECOMPRESS ---------------------------------- */

/// Unarchive a compressed tarball.
fn unarchive(archive_path: &PathBuf, target_dir: &PathBuf) {
    let file = std::fs::File::open(&archive_path).expect("could not open archive");
    let decoder = zstd::Decoder::new(file).expect("could not instantiate zstd decoder");
    tar::Archive::new(decoder)
        .unpack(target_dir)
        .expect("could not unpack archive");
}
