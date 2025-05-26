mod git;
mod release_notes;

use std::sync::Arc;

pub use release_notes::format_release_notes;

use git::GitCheckoutProgress;
use indicatif::{MultiProgress, ProgressBar};
use pixi_build_frontend::{CondaBuildReporter, CondaMetadataReporter};
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

/// A top-level reporter that combines the different reporters into one. This
/// directly implements the [`pixi_command_dispatcher::Reporter`] trait.
/// And subsequently, offloads the work to its sub progress reporters.
pub(crate) struct TopLevelProgress {
    source_checkout_reporter: GitCheckoutProgress,
}

impl TopLevelProgress {
    /// Construct a new top level progress reporter. All progress bars created
    /// by this instance are placed relative to the `anchor_pb`.
    pub fn new(multi_progress: MultiProgress, anchor_pb: ProgressBar) -> Self {
        Self {
            source_checkout_reporter: GitCheckoutProgress::new(anchor_pb, multi_progress),
        }
    }
}

impl pixi_command_dispatcher::Reporter for TopLevelProgress {
    fn as_git_reporter(&mut self) -> Option<&mut dyn pixi_command_dispatcher::GitCheckoutReporter> {
        Some(&mut self.source_checkout_reporter)
    }

    fn as_conda_solve_reporter(
        &mut self,
    ) -> Option<&mut dyn pixi_command_dispatcher::CondaSolveReporter> {
        None
    }

    fn as_pixi_solve_reporter(
        &mut self,
    ) -> Option<&mut dyn pixi_command_dispatcher::PixiSolveReporter> {
        None
    }

    fn as_pixi_install_reporter(
        &mut self,
    ) -> Option<&mut dyn pixi_command_dispatcher::PixiInstallReporter> {
        None
    }
}
