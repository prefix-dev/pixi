mod git;
mod main_progress_bar;
mod release_notes;
mod repodata_reporter;

use std::sync::{Arc, LazyLock};

use git::GitCheckoutProgress;
use indicatif::{MultiProgress, ProgressBar};
use pixi_build_frontend::{CondaBuildReporter, CondaMetadataReporter};
use pixi_command_dispatcher::{
    InstallPixiEnvironmentSpec, PixiEnvironmentSpec, ReporterContext, SolveCondaEnvironmentSpec,
    reporter::{CondaSolveId, PixiInstallId, PixiSolveId},
};
use rattler_repodata_gateway::Reporter;
pub use release_notes::format_release_notes;
use uv_configuration::RAYON_INITIALIZE;

use crate::reporters::{main_progress_bar::MainProgressBar, repodata_reporter::RepodataReporter};

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
    conda_solve_reporter: MainProgressBar<String>,
    repodata_reporter: RepodataReporter,
}

impl TopLevelProgress {
    /// Construct a new top level progress reporter. All progress bars created
    /// by this instance are placed relative to the `anchor_pb`.
    pub fn new(multi_progress: MultiProgress, anchor_pb: ProgressBar) -> Self {
        let repodata_reporter = RepodataReporter::new(
            multi_progress.clone(),
            pixi_progress::ProgressBarPlacement::Before(anchor_pb.clone()),
            "fetching repodata".to_owned(),
        );
        let conda_solve_reporter = MainProgressBar::new(
            multi_progress.clone(),
            pixi_progress::ProgressBarPlacement::Before(anchor_pb.clone()),
            "solving".to_owned(),
        );
        let source_checkout_reporter = GitCheckoutProgress::new(anchor_pb, multi_progress);
        Self {
            source_checkout_reporter,
            conda_solve_reporter,
            repodata_reporter,
        }
    }
}

impl pixi_command_dispatcher::Reporter for TopLevelProgress {
    /// Called when the command dispatcher is closing down.
    ///
    /// We want to make sure that we clean up all the progress bars.
    fn on_finished(&mut self) {
        self.on_clear()
    }

    /// Clears the current progress bars.
    fn on_clear(&mut self) {
        self.conda_solve_reporter.clear();
        self.repodata_reporter.clear();
    }

    fn as_git_reporter(&mut self) -> Option<&mut dyn pixi_command_dispatcher::GitCheckoutReporter> {
        Some(&mut self.source_checkout_reporter)
    }

    fn as_conda_solve_reporter(
        &mut self,
    ) -> Option<&mut dyn pixi_command_dispatcher::CondaSolveReporter> {
        Some(self)
    }

    fn as_pixi_solve_reporter(
        &mut self,
    ) -> Option<&mut dyn pixi_command_dispatcher::PixiSolveReporter> {
        Some(self)
    }

    fn as_pixi_install_reporter(
        &mut self,
    ) -> Option<&mut dyn pixi_command_dispatcher::PixiInstallReporter> {
        Some(self)
    }

    fn create_gateway_reporter(
        &mut self,
        _reason: Option<ReporterContext>,
    ) -> Option<Box<dyn Reporter>> {
        Some(Box::new(self.repodata_reporter.clone()))
    }
}

impl pixi_command_dispatcher::PixiInstallReporter for TopLevelProgress {
    fn on_queued(
        &mut self,
        _reason: Option<ReporterContext>,
        _env: &InstallPixiEnvironmentSpec,
    ) -> PixiInstallId {
        // Installing a pixi environment uses rayon. We only want to initialize the
        // rayon thread pool when we absolutely need it.
        LazyLock::force(&RAYON_INITIALIZE);

        PixiInstallId(0)
    }

    fn on_start(&mut self, _solve_id: PixiInstallId) {}

    fn on_finished(&mut self, _solve_id: PixiInstallId) {}
}

impl pixi_command_dispatcher::PixiSolveReporter for TopLevelProgress {
    fn on_queued(
        &mut self,
        _reason: Option<ReporterContext>,
        env: &PixiEnvironmentSpec,
    ) -> PixiSolveId {
        let id = self.conda_solve_reporter.queued(format!(
            "{} ({})",
            env.name.as_deref().unwrap_or_default(),
            env.build_environment.host_platform
        ));
        PixiSolveId(id)
    }

    fn on_start(&mut self, _solve_id: PixiSolveId) {}

    fn on_finished(&mut self, _solve_id: PixiSolveId) {}
}

impl pixi_command_dispatcher::CondaSolveReporter for TopLevelProgress {
    fn on_queued(
        &mut self,
        reason: Option<ReporterContext>,
        env: &SolveCondaEnvironmentSpec,
    ) -> CondaSolveId {
        match reason {
            Some(ReporterContext::SolvePixi(p)) => CondaSolveId(p.0),
            _ => {
                let id = self
                    .conda_solve_reporter
                    .queued(env.name.clone().unwrap_or_default());
                CondaSolveId(id)
            }
        }
    }

    fn on_start(&mut self, solve_id: CondaSolveId) {
        self.conda_solve_reporter.start(solve_id.0);
    }

    fn on_finished(&mut self, solve_id: CondaSolveId) {
        self.conda_solve_reporter.finish(solve_id.0);
    }
}
