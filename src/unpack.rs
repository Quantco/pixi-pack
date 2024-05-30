use std::path::{Path, PathBuf};

use rattler_package_streaming::fs::extract;

/* ------------------------------------------- UNPACK ------------------------------------------ */

/// Options for unpacking a pixi environment.
#[derive(Debug)]
pub struct UnpackOptions {
    pub pack_file: PathBuf,
    pub target_dir: PathBuf,
}

/// Unpack a pixi environment.
pub async fn unpack(options: UnpackOptions) -> Result<(), Box<dyn std::error::Error>> {
    let unpack_dir = options.target_dir.join("unpack");
    std::fs::create_dir_all(&unpack_dir).expect("Could not create unpack directory");
    unarchive(&options.pack_file, &unpack_dir);

    // TODO: Parallelize installation.
    let packages = collect_packages(&unpack_dir.join("environment")).unwrap();
    for package in &packages {
        extract(&package, &options.target_dir)?;
    }

    std::fs::remove_dir_all(unpack_dir).expect("Could not remove unpack directory");

    Ok(())
}

/* -------------------------------------- INSTALL PACKAGES ------------------------------------- */

/// Collect all packages in a directory.
fn collect_packages(channel: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let subdirs = channel.read_dir()?;
    let packages = subdirs
        .into_iter()
        .filter(|subdir| subdir.as_ref().is_ok_and(|subdir| subdir.path().is_dir()))
        .flat_map(|subdir| {
            let subdir = subdir.unwrap().path();
            let packages = subdir.read_dir().unwrap();
            packages
                .into_iter()
                .map(|package| package.unwrap().path())
                .filter(|package| {
                    package.extension().unwrap() == "conda"
                        || package.extension().unwrap() == "tar.bz2"
                })
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
