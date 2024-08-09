use std::{path::Path, time::Duration};

use fs_set_times::{set_times, SystemTimeSpec};
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

/// Set the modified, accessed, created time for a file.
pub fn set_default_file_times<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
    // TODO: This will only change the times for files not for directories.
    // let file_default_time = std::time::SystemTime::UNIX_EPOCH;
    // let file_times = FileTimes::new()
    //     .set_accessed(file_default_time)
    //     .set_modified(file_default_time);
    // let dest = std::fs::File::open(path)?;
    // dest.set_times(file_times)?;
    println!("Changing times for {:?}", path.as_ref());
    // TODO: Supposedly this external crate also supports changing mtime, atime for directories
    // but it doesn't fix it.
    set_times(
        path,
        Some(SystemTimeSpec::Absolute(std::time::SystemTime::UNIX_EPOCH)),
        Some(SystemTimeSpec::Absolute(std::time::SystemTime::UNIX_EPOCH)),
    )?;

    Ok(())
}
