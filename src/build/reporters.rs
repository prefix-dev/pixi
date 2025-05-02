use std::{collections::HashMap, sync::Arc, time::Duration};

use indicatif::{MultiProgress, ProgressBar};
use parking_lot::Mutex;
use pixi_build_frontend::{CondaBuildReporter, CondaMetadataReporter};
use pixi_command_queue::GitCheckoutId;
use pixi_git::resolver::RepositoryReference;

pub trait BuildMetadataReporter: CondaMetadataReporter {
    /// Reporters that the metadata has been cached.
    fn on_metadata_cached(&self, build_id: usize);

    /// Cast upwards
    fn as_conda_metadata_reporter(self: Arc<Self>) -> Arc<dyn CondaMetadataReporter>;
}

/// Noop implementation of the BuildMetadataReporter trait.
struct NoopBuildMetadataReporter;
impl CondaMetadataReporter for NoopBuildMetadataReporter {
    fn on_metadata_start(&self, _build_id: usize) -> usize {
        0
    }

    fn on_metadata_end(&self, _operation: usize) {}
}
impl BuildMetadataReporter for NoopBuildMetadataReporter {
    fn on_metadata_cached(&self, _build_id: usize) {}

    fn as_conda_metadata_reporter(self: Arc<Self>) -> Arc<dyn CondaMetadataReporter> {
        self
    }
}

pub trait BuildReporter: CondaBuildReporter {
    /// Reports that the build has been cached.
    fn on_build_cached(&self, build_id: usize);

    /// Cast upwards
    fn as_conda_build_reporter(self: Arc<Self>) -> Arc<dyn CondaBuildReporter>;
}

/// Noop implementation of the BuildReporter trait.
struct NoopBuildReporter;
impl CondaBuildReporter for NoopBuildReporter {
    fn on_build_start(&self, _build_id: usize) -> usize {
        0
    }

    fn on_build_end(&self, _operation: usize) {}

    fn on_build_output(&self, _operation: usize, _line: String) {}
}
impl BuildReporter for NoopBuildReporter {
    fn on_build_cached(&self, _build_id: usize) {}

    fn as_conda_build_reporter(self: Arc<Self>) -> Arc<dyn CondaBuildReporter> {
        self
    }
}

/// A reporter trait that it is responsible for reporting the progress of some source checkout.
pub trait SourceReporter: pixi_git::Reporter {
    /// Cast upwards
    fn as_git_reporter(self: Arc<Self>) -> Arc<dyn pixi_git::Reporter>;
}

#[derive(Default, Debug)]
struct ProgressState {
    /// A map of progress bars, by ID.
    bars: HashMap<usize, ProgressBar>,
    /// A monotonic counter for bar IDs.
    id: usize,
}

impl ProgressState {
    /// Returns a unique ID for a new progress bar.
    fn id(&mut self) -> usize {
        self.id += 1;
        self.id
    }
}

/// A reporter implementation for source checkouts.
pub struct SourceCheckoutReporter {
    /// The original progress bar.
    original_progress: ProgressBar,
    /// The multi-progress bar. Usually, this is the global multi-progress bar.
    multi_progress: MultiProgress,
    /// The state of the progress bars for each source checkout.
    progress_state: Arc<Mutex<ProgressState>>,
}

impl SourceCheckoutReporter {
    /// Creates a new source checkout reporter.
    pub fn new(original_progress: ProgressBar, multi_progress: MultiProgress) -> Self {
        Self {
            original_progress,
            multi_progress,
            progress_state: Default::default(),
        }
    }

    /// Similar to the default pixi_progress::default_progress_style, but with a spinner in front.
    pub fn spinner_style() -> indicatif::ProgressStyle {
        indicatif::ProgressStyle::with_template("  {spinner:.green} {prefix:30!} {wide_msg:.dim}")
            .expect("should be able to create a progress bar style")
    }
}

impl pixi_command_queue::GitCheckoutReporter for SourceCheckoutReporter {
    /// Called when a git checkout was queued on the [`CommandQueue`].
    fn on_checkout_queued(&mut self, env: &RepositoryReference) -> GitCheckoutId {}

    fn on_checkout_start(&mut self, checkout_id: GitCheckoutId) {
        let mut state = self.progress_state.lock();
        let id = state.id();

        let pb = self
            .multi_progress
            .insert_before(&self.original_progress, ProgressBar::hidden());
        pb.set_style(SourceCheckoutReporter::spinner_style());
        // pb.set_style(pixi_progress::default_progress_style());
        pb.set_prefix("fetching git dependencies");
        pb.set_message(format!("checking out {}@{}", url, rev));
        pb.enable_steady_tick(Duration::from_millis(100));

        state.bars.insert(id, pb);

        id
    }

    fn on_checkout_finished(&mut self, checkout_id: GitCheckoutId) {
        let mut state = self.progress_state.lock();
        let removed_pb = state
            .bars
            .remove(&index)
            .expect("the progress bar needs to be inserted for this checkout");

        removed_pb.finish_with_message(format!("checkout complete {}@{}", url, rev));
        removed_pb.finish_and_clear();
    }
}
