mod download_verify_reporter;
mod git;
mod main_progress_bar;
mod release_notes;
mod repodata_reporter;
mod run_exports;
mod sync_reporter;
pub mod uv_reporter;

use std::sync::{Arc, LazyLock};

use git::GitCheckoutProgress;
use indicatif::{MultiProgress, ProgressBar};
use main_progress_bar::MainProgressBar;
use pixi_command_dispatcher::{
    InstallPixiEnvironmentSpec, PixiEnvironmentSpec, ReporterContext, SolveCondaEnvironmentSpec,
    reporter::{
        BackendSourceBuildReporter, CondaSolveId, PixiInstallId, PixiSolveId, SourceBuildReporter,
    },
};
use pixi_spec::PixiSpec;
use rattler_repodata_gateway::{Reporter, RunExportsReporter};
pub use release_notes::format_release_notes;
use repodata_reporter::RepodataReporter;
use sync_reporter::SyncReporter;
use uv_configuration::RAYON_INITIALIZE;
// Re-export the uv_reporter types for external use
pub use uv_reporter::{UvReporter, UvReporterOptions};

/// A top-level reporter that combines the different reporters into one. This
/// directly implements the [`pixi_command_dispatcher::Reporter`] trait.
/// And subsequently, offloads the work to its sub progress reporters.
pub struct TopLevelProgress {
    source_checkout_reporter: GitCheckoutProgress,
    conda_solve_reporter: MainProgressBar<String>,
    repodata_reporter: RepodataReporter,
    sync_reporter: SyncReporter,
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
        let install_reporter = SyncReporter::new(
            multi_progress.clone(),
            pixi_progress::ProgressBarPlacement::Before(anchor_pb.clone()),
        );
        let source_checkout_reporter =
            GitCheckoutProgress::new(multi_progress.clone(), anchor_pb.clone());
        Self {
            source_checkout_reporter,
            conda_solve_reporter,
            repodata_reporter,
            sync_reporter: install_reporter,
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
        self.sync_reporter.clear();
    }

    fn as_git_reporter(&mut self) -> Option<&mut dyn pixi_command_dispatcher::GitCheckoutReporter> {
        Some(&mut self.source_checkout_reporter)
    }

    fn as_source_build_reporter(&mut self) -> Option<&mut dyn SourceBuildReporter> {
        Some(&mut self.sync_reporter)
    }

    fn as_backend_source_build_reporter(&mut self) -> Option<&mut dyn BackendSourceBuildReporter> {
        Some(&mut self.sync_reporter)
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

    fn create_install_reporter(
        &mut self,
        _reason: Option<ReporterContext>,
    ) -> Option<Box<dyn rattler::install::Reporter>> {
        Some(Box::new(self.sync_reporter.create_reporter()))
    }

    fn create_run_exports_reporter(
        &mut self,
        _reason: Option<ReporterContext>,
    ) -> Option<Arc<dyn RunExportsReporter>> {
        Some(Arc::new(run_exports::RunExportsReporter::new(
            self.repodata_reporter.clone(),
            self.sync_reporter.clone(),
        )))
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

    fn on_start(&mut self, _install_id: PixiInstallId) {}

    fn on_finished(&mut self, _install_id: PixiInstallId) {}
}

impl pixi_command_dispatcher::PixiSolveReporter for TopLevelProgress {
    fn on_queued(
        &mut self,
        _reason: Option<ReporterContext>,
        env: &PixiEnvironmentSpec,
    ) -> PixiSolveId {
        let has_direct_conda_dependency =
            env.dependencies.iter_specs().any(|(_, spec)| match spec {
                PixiSpec::Url(url) => url.is_binary(),
                PixiSpec::Path(path) => path.is_binary(),
                _ => false,
            });
        if has_direct_conda_dependency {
            // Dependencies on conda packages will trigger validating the package cache
            // which will be done using rayon. If that's the case, we need to ensure rayon
            // is initialized using the uv initialization.
            LazyLock::force(&RAYON_INITIALIZE);
        }

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
