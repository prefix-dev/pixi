use indicatif::{MultiProgress, ProgressBar};

#[derive(Default, Clone)]
pub enum ProgressBarPlacement {
    /// The progress bar is placed as the first progress bar.
    Top,
    /// The progress bar is placed as the last progress bar.
    #[default]
    Bottom,
    /// The progress bar is placed after the given progress bar.
    After(ProgressBar),
    /// The progress bar is placed before the given progress bar.
    Before(ProgressBar),
}

impl ProgressBarPlacement {
    /// Add the specified progress bar to the given multi progress instance
    pub fn insert(&self, multi_progress: MultiProgress, progress_bar: ProgressBar) -> ProgressBar {
        match self {
            ProgressBarPlacement::After(after) => multi_progress.insert_after(after, progress_bar),
            ProgressBarPlacement::Before(before) => {
                multi_progress.insert_before(before, progress_bar)
            }
            ProgressBarPlacement::Top => multi_progress.insert(0, progress_bar),
            ProgressBarPlacement::Bottom => multi_progress.add(progress_bar),
        }
    }
}
