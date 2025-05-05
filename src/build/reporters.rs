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
pub struct GitCheckoutProgress {
    /// The original progress bar.
    original_progress: ProgressBar,
    /// The multi-progress bar. Usually, this is the global multi-progress bar.
    multi_progress: MultiProgress,
    /// The state of the progress bars for each source checkout.
    progress_state: ProgressState,
    /// Refernences to the repository info
    repository_references: HashMap<GitCheckoutId, RepositoryReference>,
}

impl GitCheckoutProgress {
    /// Creates a new source checkout reporter.
    pub fn new(original_progress: ProgressBar, multi_progress: MultiProgress) -> Self {
        Self {
            original_progress,
            multi_progress,
            progress_state: Default::default(),
            repository_references: Default::default(),
        }
    }

    /// Similar to the default pixi_progress::default_progress_style, but with a spinner in front.
    pub fn spinner_style() -> indicatif::ProgressStyle {
        indicatif::ProgressStyle::with_template("  {spinner:.green} {prefix:30!} {wide_msg:.dim}")
            .expect("should be able to create a progress bar style")
    }
}

impl GitCheckoutProgress {
    pub fn get_repo_reference(&self, id: GitCheckoutId) -> &RepositoryReference {
        self.repository_references
            .get(&id)
            .expect("the progress bar needs to be inserted for this checkout")
    }
}

impl pixi_command_queue::GitCheckoutReporter for GitCheckoutProgress {
    /// Called when a git checkout was queued on the [`CommandQueue`].
    fn on_checkout_queued(&mut self, env: &RepositoryReference) -> GitCheckoutId {
        let id = self.progress_state.id();
        let checkout_id = GitCheckoutId(id);
        self.repository_references.insert(checkout_id, env.clone());
        checkout_id
    }

    fn on_checkout_start(&mut self, checkout_id: GitCheckoutId) {
        let pb = self
            .multi_progress
            .insert_after(&self.original_progress, ProgressBar::hidden());
        let repo = self.get_repo_reference(checkout_id);
        pb.set_style(GitCheckoutProgress::spinner_style());
        pb.set_prefix("fetching git dependencies");
        pb.set_message(format!(
            "checking out {}@{}",
            repo.url.as_url(),
            repo.reference
        ));
        pb.enable_steady_tick(Duration::from_millis(100));
        self.progress_state.bars.insert(checkout_id.0, pb);
    }

    fn on_checkout_finished(&mut self, checkout_id: GitCheckoutId) {
        let removed_pb = self
            .progress_state
            .bars
            .remove(&checkout_id.0)
            .expect("the progress bar needs to be inserted for this checkout");
        let repo = self.get_repo_reference(checkout_id);
        removed_pb.finish_with_message(format!(
            "checkout complete {}@{}",
            repo.url.as_url(),
            repo.reference
        ));
        removed_pb.finish_and_clear();
    }
}

/// A top-level reporter that combine the different reporters into one.
/// this directyl implements the [`pixi_command_queue::Reporter`] trait.
/// And subsequently, offloads the work to its sub progress reporters.
pub(crate) struct TopLevelProgress {
    multi_progress: MultiProgress,
    source_checkout_reporter: GitCheckoutProgress,
}

impl TopLevelProgress {
    pub fn new(multi_progress: MultiProgress) -> Self {
        let pb = multi_progress.insert_before(0, ProgressBar::hidden());
        Self {
            multi_progress: multi_progress.clone(),
            source_checkout_reporter: GitCheckoutProgress::new(pb, multi_progress),
        }
    }
}

impl pixi_command_queue::Reporter for TopLevelProgress {
    fn as_git_reporter(&mut self) -> Option<&mut dyn pixi_command_queue::GitCheckoutReporter> {
        Some(&mut self.source_checkout_reporter)
    }

    fn as_conda_solve_reporter(
        &mut self,
    ) -> Option<&mut dyn pixi_command_queue::CondaSolveReporter> {
        None
    }

    fn as_pixi_solve_reporter(&mut self) -> Option<&mut dyn pixi_command_queue::PixiSolveReporter> {
        None
    }

    fn as_pixi_install_reporter(
        &mut self,
    ) -> Option<&mut dyn pixi_command_queue::PixiInstallReporter> {
        None
    }
}
