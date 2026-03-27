use std::time::Duration;

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
