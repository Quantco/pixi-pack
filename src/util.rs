use std::path::Path;

use indicatif::{ProgressBar, ProgressStyle};
use rattler::install::Reporter;
use rattler_conda_types::RepoDataRecord;

/// Create a progress bar with default style.
pub fn create_progress_bar(length: u64) -> ProgressBar {
    ProgressBar::new(length).with_style(
        ProgressStyle::with_template(
            "[{elapsed_precise}] {bar:40.cyan/blue} {pos:>7}/{len:7} {msg}",
        )
        .expect("could not set progress style")
        .progress_chars("##-"),
    )
}

/// Progress reporter for installing packages.
pub struct InstallationProgressReporter {
    pb: ProgressBar,
}

impl InstallationProgressReporter {
    pub fn new(length: u64) -> Self {
        Self {
            pb: create_progress_bar(length),
        }
    }
}

impl Reporter for InstallationProgressReporter {
    fn on_transaction_start(
        &self,
        _transaction: &rattler::install::Transaction<
            rattler_conda_types::PrefixRecord,
            RepoDataRecord,
        >,
    ) {
    }

    fn on_transaction_operation_start(&self, _operation: usize) {}
    fn on_download_start(&self, cache_entry: usize) -> usize {
        cache_entry
    }

    fn on_download_completed(&self, _download_idx: usize) {}

    fn on_link_start(&self, operation: usize, _record: &RepoDataRecord) -> usize {
        operation
    }

    fn on_link_complete(&self, _index: usize) {}

    fn on_transaction_operation_complete(&self, _operation: usize) {
        self.pb.inc(1);
    }

    fn on_populate_cache_start(&self, operation: usize, _record: &RepoDataRecord) -> usize {
        operation
    }

    fn on_validate_start(&self, cache_entry: usize) -> usize {
        cache_entry
    }

    fn on_validate_complete(&self, _validate_idx: usize) {}

    fn on_download_progress(&self, _download_idx: usize, _progress: u64, _total: Option<u64>) {}

    fn on_populate_cache_complete(&self, _cache_entry: usize) {}

    fn on_unlink_start(
        &self,
        operation: usize,
        _record: &rattler_conda_types::PrefixRecord,
    ) -> usize {
        operation
    }

    fn on_unlink_complete(&self, _index: usize) {}

    fn on_transaction_complete(&self) {
        self.pb.finish_and_clear();
    }
}

/// Get the size of a file or directory in bytes.
pub fn get_size<P: AsRef<Path>>(path: P) -> std::io::Result<u64> {
    let metadata = std::fs::metadata(&path)?;
    let mut size = metadata.len();
    if metadata.is_dir() {
        for entry in std::fs::read_dir(&path)? {
            size += get_size(entry?.path())?;
        }
    }
    Ok(size)
}
