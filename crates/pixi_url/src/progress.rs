use indicatif::{ProgressBar, ProgressStyle};

/// Abstraction over progress reporting so callers can hook into their own UI.
pub trait ProgressHandler: Send + Sync {
    /// Adds a progress bar to the underlying renderer, returning the wrapped bar.
    fn add_progress_bar(&self, bar: ProgressBar) -> ProgressBar;

    /// Returns the default style to use for byte-based progress.
    fn default_bytes_style(&self) -> ProgressStyle {
        default_bytes_style()
    }
}

/// A no-op progress handler that simply returns the provided progress bars.
#[derive(Clone, Default)]
pub struct NoProgressHandler;

impl ProgressHandler for NoProgressHandler {
    fn add_progress_bar(&self, bar: ProgressBar) -> ProgressBar {
        bar
    }

    fn default_bytes_style(&self) -> ProgressStyle {
        default_bytes_style()
    }
}

fn default_bytes_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{prefix:<18} [{elapsed_precise}] {wide_bar} {bytes}/{total_bytes}",
    )
    .unwrap()
    .progress_chars("━ ")
}
