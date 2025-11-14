use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

use crate::ProgressBarPlacement;

/// A display that shows a rolling window of the last N lines of output.
///
/// This is useful for streaming output (like build logs) where you want to show
/// recent activity without cluttering the terminal. Each line is displayed as a
/// separate progress bar using `{wide_msg}` to prevent wrapping.
pub struct RollingLogDisplay {
    /// Progress bars for visible lines (up to max_lines)
    lines: Vec<ProgressBar>,
    /// Full log buffer (for failure case)
    full_log: Vec<String>,
    /// MultiProgress reference
    multi_progress: MultiProgress,
    /// Placement strategy for the progress bars
    placement: ProgressBarPlacement,
    /// Maximum visible lines
    max_lines: usize,
}

impl RollingLogDisplay {
    /// Create a new rolling log display with default max lines (6).
    ///
    /// # Arguments
    /// * `multi_progress` - The MultiProgress instance to add progress bars to
    /// * `placement` - Where to place the first progress bar
    pub fn new(multi_progress: MultiProgress, placement: ProgressBarPlacement) -> Self {
        Self::with_max_lines(multi_progress, placement, 6)
    }

    /// Create a new rolling log display with custom max lines.
    ///
    /// # Arguments
    /// * `multi_progress` - The MultiProgress instance to add progress bars to
    /// * `placement` - Where to place the first progress bar
    /// * `max_lines` - Maximum number of lines to display at once
    pub fn with_max_lines(
        multi_progress: MultiProgress,
        placement: ProgressBarPlacement,
        max_lines: usize,
    ) -> Self {
        Self {
            lines: Vec::new(),
            full_log: Vec::new(),
            multi_progress,
            placement,
            max_lines,
        }
    }

    /// Push a new line to the display.
    ///
    /// If the display is not yet at capacity, a new progress bar is created.
    /// If at capacity, the oldest progress bar is updated and moved to the end.
    pub fn push_line(&mut self, line: impl Into<String>) {
        let line = line.into();

        // Always buffer the full log
        self.full_log.push(line.clone());

        // If we haven't reached max capacity, create a new progress bar
        if self.lines.len() < self.max_lines {
            // Create as hidden first
            let pb = ProgressBar::hidden();

            // Add to MultiProgress using placement strategy
            let pb = if self.lines.is_empty() {
                self.placement.insert(self.multi_progress.clone(), pb)
            } else {
                let last_bar = self.lines.last().unwrap();
                ProgressBarPlacement::After(last_bar.clone())
                    .insert(self.multi_progress.clone(), pb)
            };

            // Now configure the style and message (dimmed)
            pb.set_style(
                ProgressStyle::with_template("{wide_msg:.dim}")
                    .expect("failed to set progress bar template"),
            );
            pb.set_message(line);

            self.lines.push(pb);
        } else {
            // At capacity - update all bars to show the last N lines in correct order
            // Get the last max_lines from the full log
            let start_idx = self.full_log.len().saturating_sub(self.max_lines);
            for (i, pb) in self.lines.iter().enumerate() {
                if let Some(msg) = self.full_log.get(start_idx + i) {
                    pb.set_message(msg.clone());
                }
            }
        }
    }

    /// Get a reference to all buffered log lines.
    ///
    /// This returns the complete log history, useful for displaying
    /// full output on failure.
    pub fn full_log(&self) -> &[String] {
        &self.full_log
    }

    /// Consume self and return the full log buffer without copying.
    ///
    /// This also finishes and clears all progress bars.
    pub fn into_full_log(self) -> Vec<String> {
        for pb in self.lines {
            pb.finish_and_clear();
        }
        self.full_log
    }

    /// Clear the display and remove all progress bars, consuming self.
    ///
    /// This finishes and clears all progress bars, removing them from the display.
    pub fn finish(self) {
        for pb in self.lines {
            pb.finish_and_clear();
        }
    }

    /// Clear the progress bars from display but keep the log buffer.
    ///
    /// This removes all progress bars from the terminal but retains the full log
    /// in memory for later retrieval.
    pub fn clear_display(&mut self) {
        for pb in self.lines.drain(..) {
            pb.finish_and_clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_push() {
        let mp = MultiProgress::new();
        let mut display = RollingLogDisplay::new(mp, ProgressBarPlacement::Bottom);

        display.push_line("line 1");
        display.push_line("line 2");
        display.push_line("line 3");

        assert_eq!(display.full_log().len(), 3);
        assert_eq!(display.lines.len(), 3);
        assert_eq!(display.full_log(), &["line 1", "line 2", "line 3"]);
    }

    #[test]
    fn test_max_lines() {
        let mp = MultiProgress::new();
        let mut display = RollingLogDisplay::with_max_lines(mp, ProgressBarPlacement::Bottom, 3);

        display.push_line("line 1");
        display.push_line("line 2");
        display.push_line("line 3");

        // At capacity
        assert_eq!(display.lines.len(), 3);

        display.push_line("line 4");

        // Still at capacity (3 bars), but full log has all 4
        assert_eq!(display.lines.len(), 3);
        assert_eq!(display.full_log().len(), 4);
    }

    #[test]
    fn test_clear_display() {
        let mp = MultiProgress::new();
        let mut display = RollingLogDisplay::new(mp, ProgressBarPlacement::Bottom);

        display.push_line("line 1");
        display.push_line("line 2");

        assert_eq!(display.lines.len(), 2);

        display.clear_display();

        assert_eq!(display.lines.len(), 0);
        assert_eq!(display.full_log().len(), 2); // Log still preserved
    }

    #[test]
    fn test_finish() {
        let mp = MultiProgress::new();
        let mut display = RollingLogDisplay::new(mp, ProgressBarPlacement::Bottom);

        display.push_line("line 1");
        display.push_line("line 2");

        let full_log_len = display.full_log().len();
        display.finish();

        assert_eq!(full_log_len, 2);
    }
}
