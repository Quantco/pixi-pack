use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use async_compression::tokio::bufread::ZstdDecoder;
use rattler_conda_types::Platform;
use rattler_shell::shell::ShellEnum;
use tokio::fs::{self, create_dir_all};
use tokio_tar::Archive;
use url::Url;

use std::borrow::Borrow;

use futures::{stream, StreamExt, TryStreamExt};
use rattler::install::{link_package, Transaction};
use rattler::install::{InstallDriver, InstallOptions};
use rattler_conda_types::package::{IndexJson, PackageFile};
use rattler_conda_types::{PackageRecord, PrefixRecord, RepoDataRecord};
use rattler_package_streaming::tokio::fs::extract;
use rattler_shell::activation::ActivationVariables;
use rattler_shell::activation::Activator;
use rattler_shell::activation::PathModificationBehavior;
use rattler_shell::shell::Shell;
use tokio_stream::wrappers::ReadDirStream;

use crate::{PixiPackMetadata, DEFAULT_PIXI_PACK_VERSION, PIXI_PACK_METADATA_PATH};

/* ------------------------------------------- UNPACK ------------------------------------------ */

/// Options for unpacking a pixi environment.
#[derive(Debug)]
pub struct UnpackOptions {
    pub pack_file: PathBuf,
    pub output_directory: PathBuf,
    pub shell: ShellEnum,
}

/// Unpack a pixi environment.
pub async fn unpack(options: UnpackOptions) -> Result<()> {
    // TODO: Dont use static dir here but a temp dir
    let unpack_dir =
        tempfile::tempdir().map_err(|e| anyhow!("could not create temporary directory: {e}"))?;
    create_dir_all(&unpack_dir)
        .await
        .map_err(|e| anyhow!("Could not create unpack directory: {}", e))?;

    unarchive(&options.pack_file, &unpack_dir.path())
        .await
        .map_err(|e| anyhow!("Could not unarchive: {}", e))?;

    // Read pixi-pack metadata file
    let metadata_file = unpack_dir.path().join(PIXI_PACK_METADATA_PATH);

    let metadata_contents = tokio::fs::read_to_string(&metadata_file)
        .await
        .map_err(|e| anyhow!("Could not read metadata file: {}", e))?;

    let metadata: PixiPackMetadata = serde_json::from_str(&metadata_contents)?;

    if metadata.version != DEFAULT_PIXI_PACK_VERSION {
        anyhow::bail!("Unsupported pixi-pack version: {}", metadata.version);
    }
    if metadata.platform != Platform::current() {
        anyhow::bail!("The pack was created for a different platform");
    }

    let target_prefix = options.output_directory.join("env");

    install(&target_prefix, &unpack_dir.path().join("pkgs")).await?;

    create_activation_script(&options.output_directory, &target_prefix, options.shell)
        .await
        .map_err(|e| anyhow!("could not create activation script: {}", e))?;

    Ok(())
}

/// Unarchive a compressed tarball.
async fn unarchive(archive_path: &Path, target_dir: &Path) -> Result<()> {
    let file = fs::File::open(archive_path)
        .await
        .map_err(|e| anyhow!("could not open archive {:#?}: {}", archive_path, e))?;

    let reader = tokio::io::BufReader::new(file);

    let decocder = ZstdDecoder::new(reader);

    let mut archive = Archive::new(decocder);

    archive
        .unpack(target_dir)
        .await
        .map_err(|e| anyhow!("could not unpack archive: {}", e))?;

    Ok(())
}

// Transaction::from_current_and_desired requires New to implement AsRef<PackageRecord>
// but PackageRecord does not implement AsRef<PackageRecord>, so we need to wrap it
// in a struct that does as we can't implement traits for types we don't own.
struct WrappedPackageRecord(PackageRecord);

impl AsRef<PackageRecord> for WrappedPackageRecord {
    fn as_ref(&self) -> &PackageRecord {
        &self.0
    }
}

// We just need a type to make the compiler happy
// this is actually never constructed
struct WrappedOld {}

impl AsRef<WrappedPackageRecord> for WrappedOld {
    fn as_ref(&self) -> &WrappedPackageRecord {
        unimplemented!()
    }
}

impl<'a> Borrow<PrefixRecord> for &'a WrappedOld {
    fn borrow(&self) -> &PrefixRecord {
        unimplemented!()
    }
}

impl<'a> AsRef<PackageRecord> for &'a WrappedOld {
    fn as_ref(&self) -> &PackageRecord {
        unimplemented!()
    }
}

async fn install(target_prefix: &Path, archived_package_dir: &Path) -> Result<()> {
    // TODO: this will not execute link scripts
    let target_platform = Platform::current();

    let driver = InstallDriver::default();

    let package_dir =
        tempfile::tempdir().map_err(|e| anyhow!("could not create temporary directory: {e}"))?;

    let package_dir_path = package_dir.path().to_owned();

    let packages = fs::read_dir(archived_package_dir)
        .await
        .map_err(|e| anyhow!("could not read directory: {e}"))?;

    let stream = ReadDirStream::new(packages);

    // Step 1: Extract all packages and collect the PackageRecords
    let package_records: Vec<(PackageRecord, PathBuf)> = stream
        .map_err(|e| anyhow!("could not read directory: {e}"))
        .map_ok(|package_file| {
            let package_dir_path = package_dir_path.clone();
            async move {
                let filename = package_file.file_name();

                let package_dir = package_dir_path.join(filename);

                tracing::debug!("Extracting package: {:?}", package_file.path());

                extract(&package_file.path(), &package_dir).await?;

                let index_json = IndexJson::from_package_directory(&package_dir)
                    .map_err(|e| anyhow!("could not read index.json: {e}"))?;

                let package_record =
                    PackageRecord::from_index_json(index_json, None, None, None)
                        .map_err(|e| anyhow!("could not create package record: {e}"))?;

                Ok::<(PackageRecord, PathBuf), anyhow::Error>((package_record, package_dir))
            }
        })
        .try_buffer_unordered(50)
        .try_collect()
        .await?;

    // Step 2: Build up the transaction
    let transaction: Transaction<&WrappedOld, WrappedPackageRecord> =
        Transaction::from_current_and_desired(
            &[],
            package_records
                .iter()
                .map(|(p, _)| WrappedPackageRecord(p.clone())),
            target_platform,
        )
        .map_err(|e| anyhow!("could not create transaction: {e}"))?;

    // Step 3: Preprocess, at the moment this is a no-op as there are no packages installed and so there are no spre-unlink scripts
    driver
        .pre_process(&transaction, target_prefix)
        .map_err(|e| anyhow!("preprocessing failed: {e}"))?;

    let python_info = &transaction.python_info;

    // Step 4: Link packages
    stream::iter(package_records.into_iter())
        .map(Ok) // Lift to TryStreamExt
        .try_for_each_concurrent(150, |(record, dir)| async {
            let dir = dir;
            let options = InstallOptions {
                python_info: python_info.clone(),
                ..Default::default()
            };

            let file_name = dir.file_name().unwrap().to_str().unwrap().to_string();

            let paths = link_package(&dir, target_prefix, &driver, options)
                .await
                .map_err(|e| anyhow!("could not link package {}: {}", &file_name, e))?;

            let conda_meta_path = target_prefix.join("conda-meta");
            create_dir_all(&conda_meta_path)
                .await
                .map_err(|e| anyhow!("could not create conda-meta directory: {e}"))?;

            let url = Url::parse(&format!("file:///{}", &file_name)).expect("could not create url");

            let repodata_record = RepoDataRecord {
                package_record: record,
                file_name,
                url,
                channel: "local".to_string(),
            };

            let prefix_record =
                PrefixRecord::from_repodata_record(repodata_record, None, None, paths, None, None);

            prefix_record
                .write_to_path(conda_meta_path.join(prefix_record.file_name()), true)
                .map_err(|e| anyhow!("could not write package record: {e}"))?;

            Ok::<(), anyhow::Error>(())
        })
        .await?;

    // Step 5: Postprocess, this will run the post-link scripts
    driver
        .post_process(&transaction, target_prefix)
        .map_err(|e| anyhow!("postprocessing failed: {e}"))?;

    // Step 7: Create conda-meta/history
    let history_path = target_prefix.join("conda-meta");

    fs::write(
        history_path.join("history"),
        "// not relevant for pixi but for `conda run -p`",
    )
    .await
    .map_err(|e| anyhow!("Could not write history file: {}", e))?;

    Ok(())
}

async fn create_activation_script(
    destination: &Path,
    prefix: &Path,
    shell: ShellEnum,
) -> Result<()> {
    let file_extension = shell.extension();
    let activate_path = destination.join(format!("activate.{}", file_extension));
    let activator = Activator::from_path(prefix, shell, Platform::current())?;

    let result = activator.activation(ActivationVariables {
        conda_prefix: None,
        path: None,
        path_modification_behavior: PathModificationBehavior::Prepend,
    })?;

    let contents = result.script.contents()?;
    fs::write(activate_path, contents)
        .await
        .map_err(|e| anyhow!("Could not write activate script: {}", e))?;

    Ok(())
}
