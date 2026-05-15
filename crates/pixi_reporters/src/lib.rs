mod download_verify_reporter;
mod git;
mod main_progress_bar;
mod release_notes;
mod repodata_reporter;
mod sync_reporter;
pub mod uv_reporter;

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
};

use git::GitCheckoutProgress;
use indicatif::{MultiProgress, ProgressBar};
use main_progress_bar::MainProgressBar;
use parking_lot::Mutex;
use pixi_command_dispatcher::{
    CommandDispatcherBuilder, InstallPixiEnvironmentSpec, PixiSolveEnvironmentSpec,
    SolveCondaEnvironmentSpec,
};
use pixi_compute_reporters::{OperationId, OperationRegistry};
pub use release_notes::format_release_notes;
use repodata_reporter::RepodataReporter;
use sync_reporter::SyncReporter;
use uv_configuration::RAYON_INITIALIZE;
// Re-export the uv_reporter types for external use
pub use uv_reporter::{UvReporter, UvReporterOptions};

/// Top-level progress reporter for `pixi`'s CLI. Use
/// [`Self::register_with`] to wire it into a [`CommandDispatcherBuilder`];
/// keep the `Arc` around to call [`Self::on_clear`] between phases.
pub struct TopLevelProgress {
    registry: Arc<OperationRegistry>,
    source_checkout_reporter: Arc<GitCheckoutProgress>,
    conda_solve_reporter: MainProgressBar<String>,
    /// `OperationId` → bar slot in `conda_solve_reporter`. Lets
    /// `on_started` / `on_finished` find the bar created at `on_queued`.
    solve_bars: Mutex<HashMap<OperationId, usize>>,
    repodata_reporter: RepodataReporter,
    sync_reporter: SyncReporter,
}

impl TopLevelProgress {
    /// Build an `Arc<Self>` anchored on the global multi-progress with
    /// a fresh [`OperationRegistry`].
    pub fn from_global() -> Arc<Self> {
        let multi_progress = pixi_progress::global_multi_progress();
        let anchor_pb = multi_progress.add(ProgressBar::hidden());
        Arc::new(Self::new(
            OperationRegistry::new(),
            multi_progress,
            anchor_pb,
        ))
    }

    /// The registry used by this progress reporter to allocate ids and
    /// look up parent relationships.
    pub fn registry(&self) -> &Arc<OperationRegistry> {
        &self.registry
    }

    /// Construct a new top level progress reporter. All progress bars created
    /// by this instance are placed relative to the `anchor_pb`.
    pub fn new(
        registry: Arc<OperationRegistry>,
        multi_progress: MultiProgress,
        anchor_pb: ProgressBar,
    ) -> Self {
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
            registry.clone(),
            multi_progress.clone(),
            pixi_progress::ProgressBarPlacement::Before(anchor_pb.clone()),
        );
        let source_checkout_reporter = Arc::new(GitCheckoutProgress::new(
            registry.clone(),
            multi_progress.clone(),
            anchor_pb.clone(),
        ));
        Self {
            registry,
            source_checkout_reporter,
            conda_solve_reporter,
            solve_bars: Mutex::new(HashMap::new()),
            repodata_reporter,
            sync_reporter: install_reporter,
        }
    }

    /// Register every sub-reporter this instance owns into the
    /// dispatcher builder. The git-checkout slot gets its inner
    /// progress Arc directly; the rest go through `self`.
    pub fn register_with(
        self: Arc<Self>,
        builder: CommandDispatcherBuilder,
    ) -> CommandDispatcherBuilder {
        let backend_source_build_reporter: Arc<
            dyn pixi_command_dispatcher::BackendSourceBuildReporter,
        > = Arc::new(self.sync_reporter.clone());
        builder
            .with_pixi_solve_reporter(self.clone())
            .with_conda_solve_reporter(self.clone())
            .with_pixi_install_reporter(self.clone())
            .with_instantiate_backend_reporter(self.clone())
            .with_git_checkout_reporter(self.source_checkout_reporter.clone())
            .with_backend_source_build_reporter(backend_source_build_reporter)
            .with_gateway_reporter(self.clone())
    }

    /// Clear the current progress bars without tearing down the reporter.
    pub fn on_clear(&self) {
        self.conda_solve_reporter.clear();
        self.repodata_reporter.clear();
        self.sync_reporter.clear();
    }

    fn alloc(&self) -> OperationId {
        self.registry.allocate()
    }

    fn solve_bar(&self, id: OperationId) -> Option<usize> {
        self.solve_bars.lock().get(&id).copied()
    }
}

impl pixi_command_dispatcher::PixiInstallReporter for TopLevelProgress {
    fn on_queued(&self, _env: &InstallPixiEnvironmentSpec) -> OperationId {
        self.alloc()
    }

    fn on_started(&self, _install_id: OperationId) {}

    fn on_finished(&self, _install_id: OperationId) {}

    fn create_install_reporter(&self) -> Option<Box<dyn rattler::install::Reporter>> {
        Some(Box::new(self.sync_reporter.create_reporter()))
    }
}

impl pixi_command_dispatcher::InstantiateBackendReporter for TopLevelProgress {
    fn on_queued(&self, _spec: &pixi_build_discovery::JsonRpcBackendSpec) -> OperationId {
        self.alloc()
    }

    fn on_started(&self, _id: OperationId) {}

    fn on_finished(&self, _id: OperationId) {}

    fn create_install_reporter(&self) -> Option<Box<dyn rattler::install::Reporter>> {
        Some(Box::new(self.sync_reporter.create_reporter()))
    }
}

impl pixi_command_dispatcher::PixiSolveReporter for TopLevelProgress {
    fn on_queued(&self, env: &PixiSolveEnvironmentSpec) -> OperationId {
        if env.has_direct_conda_dependency {
            // Dependencies on conda packages trigger package-cache
            // validation via rayon; ensure it's initialized through the
            // uv path before the validation work starts.
            LazyLock::force(&RAYON_INITIALIZE);
        }

        let id = self.alloc();
        let bar = self
            .conda_solve_reporter
            .queued(format!("{} ({})", env.name, env.platform));
        self.solve_bars.lock().insert(id, bar);
        id
    }

    fn on_started(&self, _solve_id: OperationId) {}

    fn on_finished(&self, solve_id: OperationId) {
        self.solve_bars.lock().remove(&solve_id);
    }
}

impl pixi_command_dispatcher::CondaSolveReporter for TopLevelProgress {
    fn on_queued(&self, env: &SolveCondaEnvironmentSpec) -> OperationId {
        // Reuse the parent pixi-solve's bar slot when one is active so
        // a conda solve nested inside a pixi solve renders as the same
        // entry rather than a fresh row.
        let id = self.alloc();
        let parent_bar = self
            .registry
            .ancestors(id)
            .find_map(|ancestor| self.solve_bar(ancestor));
        let bar = match parent_bar {
            Some(bar) => bar,
            None => self
                .conda_solve_reporter
                .queued(env.name.clone().unwrap_or_default()),
        };
        self.solve_bars.lock().insert(id, bar);
        id
    }

    fn on_started(&self, solve_id: OperationId) {
        if let Some(bar) = self.solve_bar(solve_id) {
            self.conda_solve_reporter.start(bar);
        }
    }

    fn on_finished(&self, solve_id: OperationId) {
        if let Some(bar) = self.solve_bars.lock().remove(&solve_id) {
            self.conda_solve_reporter.finish(bar);
        }
    }
}

impl pixi_command_dispatcher::GatewayReporter for TopLevelProgress {
    fn create_gateway_reporter(
        &self,
        _op_id: OperationId,
    ) -> Option<Box<dyn rattler_repodata_gateway::Reporter>> {
        Some(Box::new(self.repodata_reporter.clone()))
    }
}
