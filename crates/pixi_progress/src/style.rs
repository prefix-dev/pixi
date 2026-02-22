//! Defines some functions that can be used to style indicatif progress bars.

/// The characters to use to show progress in the progress bar.
const DEFAULT_PROGRESS_CHARS: &str = "━━╾─";

/// The characters that make up animation of a spinner that should be used when
/// the progress bar is currently making progress.
const DEFAULT_RUNNING_SPINNER_CHARS: &str = "⠁⠁⠉⠙⠚⠒⠂⠂⠒⠲⠴⠤⠄⠄⠤⠠⠠⠤⠦⠖⠒⠐⠐⠒⠓⠋⠉⠈⠈ ";

/// The characters that make up an animation of a spinner that should be used
/// when the progress bar is currently paused or not making progress.
const DEFAULT_PAUSED_SPINNER_CHARS: &str = "▪▪";

/// Returns the "tick chars" that are used to represent a spinner animation of a
/// progress bar.
pub fn tick_chars(active: bool) -> &'static str {
    if active {
        DEFAULT_RUNNING_SPINNER_CHARS
    } else {
        DEFAULT_PAUSED_SPINNER_CHARS
    }
}

/// Returns the "progress chars" that are used to render a progress bar.
pub fn progress_chars(_active: bool) -> &'static str {
    DEFAULT_PROGRESS_CHARS
}
