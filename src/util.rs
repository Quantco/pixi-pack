use std::{path::Path, time::Duration};

use indicatif::{ProgressBar, ProgressStyle};

/// Progress reporter that wraps a progress bar with default styles.
pub struct ProgressReporter {
    pub pb: ProgressBar,
}

impl ProgressReporter {
    pub fn new(length: u64) -> Self {
        let pb = ProgressBar::new(length).with_style(
            ProgressStyle::with_template("[{elapsed_precise}] {bar:40.cyan/blue} {msg}")
                .expect("could not set progress style")
                .progress_chars("##-"),
        );
        pb.enable_steady_tick(Duration::from_millis(500));
        Self { pb }
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
