mod download_verify_reporter;
mod git;
mod main_progress_bar;
mod release_notes;
mod repodata_reporter;
mod sync_reporter;
pub mod uv_reporter;

use std::{collections::HashMap, sync::Arc};

use futures::stream::StreamExt;
use git::GitCheckoutProgress;
use indicatif::{MultiProgress, ProgressBar};
use main_progress_bar::MainProgressBar;
use parking_lot::Mutex;
use pixi_command_dispatcher::{
    BuildBackendMetadataInner, BuildBackendMetadataReporter, CommandDispatcherBuilder,
    InstallPixiEnvironmentSpec, PixiSolveEnvironmentSpec, SolveCondaEnvironmentSpec,
};
use pixi_compute_reporters::{OperationId, OperationRegistry};
pub use release_notes::format_release_notes;
use repodata_reporter::RepodataReporter;
pub use repodata_reporter::{
    channel_notices_cache_dir, display_channel_notices, queue_channel_notice,
};
use sync_reporter::SyncReporter;
use uv_threads::initialize_rayon_once;
// Re-export the uv_reporter types for external use
pub use uv_reporter::{UvReporter, UvReporterOptions};

/// Top-level progress reporter for `pixi`'s CLI. Use
/// [`Self::register_with`] to wire it into a [`CommandDispatcherBuilder`];
/// keep the `Arc` around to call [`Self::on_clear`] between phases.
pub struct TopLevelProgress {
    registry: Arc<OperationRegistry>,
    source_checkout_reporter: Arc<GitCheckoutProgress>,
    conda_solve_reporter: MainProgressBar<String>,
    /// Pixi solve `OperationId` → its bar slot in `conda_solve_reporter`,
    /// until the pixi solve's conda solve claims it.
    pixi_solve_bars: Mutex<HashMap<OperationId, usize>>,
    /// Conda solve `OperationId` → its bar in `conda_solve_reporter`. Lets
    /// `on_started` / `on_finished` find the bar chosen at `on_queued`.
    solve_bars: Mutex<HashMap<OperationId, CondaSolveBar>>,
    /// `OperationId` → bar slot in `conda_solve_reporter` for build-backend
    /// metadata fetches. Kept separate from `solve_bars` so they can be
    /// resolved independently in the reporter callbacks.
    backend_metadata_bars: Mutex<HashMap<OperationId, usize>>,
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
            pixi_solve_bars: Mutex::new(HashMap::new()),
            solve_bars: Mutex::new(HashMap::new()),
            backend_metadata_bars: Mutex::new(HashMap::new()),
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
            .with_build_backend_metadata_reporter(self.clone())
            .with_git_checkout_reporter(self.source_checkout_reporter.clone())
            .with_backend_source_build_reporter(backend_source_build_reporter)
            .with_gateway_reporter(self.clone())
    }

    /// Clears this reporter's bars when the returned guard is dropped,
    /// including when the work ends in an error.
    ///
    /// A reporter that outlives the work it reports on -- one held for the
    /// duration of a command rather than a single call -- leaves finished bars
    /// rendered on stderr, and whatever the caller prints next is written over
    /// them. Hold this guard across the work to avoid that.
    pub fn clear_when_done(progress: Option<&Arc<Self>>) -> ClearWhenDone {
        ClearWhenDone(progress.cloned())
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

    #[cfg(test)]
    fn solve_bar(&self, id: OperationId) -> Option<usize> {
        match self.solve_bars.lock().get(&id)? {
            CondaSolveBar::Bar(bar) => Some(*bar),
            CondaSolveBar::Unclaimed { .. } => None,
        }
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
            initialize_rayon_once();
        }

        let id = self.alloc();
        let bar = self
            .conda_solve_reporter
            .queued(format!("{} ({})", env.name, env.platform));
        self.pixi_solve_bars.lock().insert(id, bar);
        id
    }

    fn on_started(&self, _solve_id: OperationId) {}

    fn on_finished(&self, solve_id: OperationId) {
        // A claimed bar is finished by the conda solve that claimed it.
        // Otherwise no conda solve ran under this pixi solve, e.g. because it
        // failed earlier, its conda solve was cancelled before starting, or
        // its conda solve was shared with another pixi solve, so finish the
        // bar here or it would never be cleared.
        if let Some(bar) = self.pixi_solve_bars.lock().remove(&solve_id) {
            self.conda_solve_reporter.finish(bar);
        }
    }
}

impl pixi_command_dispatcher::CondaSolveReporter for TopLevelProgress {
    fn on_queued(&self, env: &SolveCondaEnvironmentSpec) -> OperationId {
        // A pixi solve's own conda solve renders as the pixi solve's entry
        // rather than a fresh row. Only a direct child qualifies: solves
        // further down, such as a build-backend environment, are separate
        // work, and sharing the bar would finish it twice.
        let id = self.alloc();
        let name = env.name.clone().unwrap_or_default();
        let entry = match self.registry.parent_of(id) {
            Some(pixi_solve) if self.pixi_solve_bars.lock().contains_key(&pixi_solve) => {
                CondaSolveBar::Unclaimed { pixi_solve, name }
            }
            _ => CondaSolveBar::Bar(self.conda_solve_reporter.queued(name)),
        };
        self.solve_bars.lock().insert(id, entry);
        id
    }

    fn on_started(&self, solve_id: OperationId) {
        let Some(entry) = self.solve_bars.lock().remove(&solve_id) else {
            return;
        };
        let bar = match entry {
            CondaSolveBar::Bar(bar) => bar,
            CondaSolveBar::Unclaimed { pixi_solve, name } => {
                // Claim the pixi solve's bar, taking over finishing it, unless
                // the pixi solve has already finished or another conda solve
                // claimed it first.
                let claimed = self.pixi_solve_bars.lock().remove(&pixi_solve);
                claimed.unwrap_or_else(|| self.conda_solve_reporter.queued(name))
            }
        };
        self.solve_bars
            .lock()
            .insert(solve_id, CondaSolveBar::Bar(bar));
        self.conda_solve_reporter.start(bar);
    }

    fn on_finished(&self, solve_id: OperationId) {
        if let Some(CondaSolveBar::Bar(bar)) = self.solve_bars.lock().remove(&solve_id) {
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

impl BuildBackendMetadataReporter for TopLevelProgress {
    fn on_queued(&self, env: &BuildBackendMetadataInner) -> OperationId {
        let id = self.alloc();
        let bar = self
            .conda_solve_reporter
            .queued(format!("backend metadata: {}", env.manifest_source));
        self.backend_metadata_bars.lock().insert(id, bar);
        id
    }

    fn on_started(
        &self,
        id: OperationId,
        backend_output_stream: Box<dyn futures::Stream<Item = String> + Unpin + Send>,
    ) {
        if let Some(bar) = self.backend_metadata_bars.lock().get(&id).copied() {
            self.conda_solve_reporter.start(bar);
        }
        // The backend's stdout/stderr is interesting at -vvv, but we don't
        // want to interleave it into the progress bar. Drain the stream in
        // the background so the producer never blocks; the lines are
        // already visible to anyone running with `RUST_LOG`-level tracing.
        tokio::spawn(async move {
            let mut stream = backend_output_stream;
            while stream.next().await.is_some() {}
        });
    }

    fn on_finished(&self, id: OperationId, _failed: bool) {
        if let Some(bar) = self.backend_metadata_bars.lock().remove(&id) {
            self.conda_solve_reporter.finish(bar);
        }
    }
}

/// The bar of a conda solve.
enum CondaSolveBar {
    /// Its own entry, or the pixi solve bar it claimed.
    Bar(usize),
    /// A pixi solve's own conda solve that has not started yet. It claims the
    /// pixi solve's bar only when it starts: one cancelled while waiting for a
    /// solver never reports finishing, so the pixi solve must finish its bar.
    Unclaimed {
        pixi_solve: OperationId,
        name: String,
    },
}

/// Clears the progress bars of the reporter it was created from once it goes
/// out of scope. See [`TopLevelProgress::clear_when_done`].
#[must_use = "the bars are cleared when this guard drops, so it must be bound"]
pub struct ClearWhenDone(Option<Arc<TopLevelProgress>>);

impl Drop for ClearWhenDone {
    fn drop(&mut self) {
        if let Some(progress) = &self.0 {
            progress.on_clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget};
    use itertools::Itertools;
    use pixi_command_dispatcher::{
        CondaSolveReporter, PixiSolveEnvironmentSpec, PixiSolveReporter, SolveCondaEnvironmentSpec,
    };
    use pixi_compute_reporters::{OperationId, OperationRegistry};

    use super::TopLevelProgress;

    fn progress() -> TopLevelProgress {
        let multi_progress = MultiProgress::with_draw_target(ProgressDrawTarget::hidden());
        let anchor_pb = multi_progress.add(ProgressBar::hidden());
        TopLevelProgress::new(OperationRegistry::new(), multi_progress, anchor_pb)
    }

    fn queue_pixi_solve(progress: &TopLevelProgress) -> OperationId {
        let id = PixiSolveReporter::on_queued(
            progress,
            &PixiSolveEnvironmentSpec {
                name: "default".to_owned(),
                platform: "linux-64".to_owned(),
                has_direct_conda_dependency: false,
            },
        );
        PixiSolveReporter::on_started(progress, id);
        id
    }

    fn queue_conda_solve(progress: &TopLevelProgress) -> OperationId {
        CondaSolveReporter::on_queued(progress, &SolveCondaEnvironmentSpec::default())
    }

    fn pixi_bar(progress: &TopLevelProgress, id: OperationId) -> usize {
        progress.pixi_solve_bars.lock()[&id]
    }

    /// Regression test for <https://github.com/prefix-dev/pixi/issues/7369>.
    #[test]
    fn nested_conda_solve_gets_its_own_bar() {
        let progress = progress();
        let pixi = queue_pixi_solve(&progress);
        let pixi_bar = pixi_bar(&progress, pixi);
        block_on(pixi.scope_active(async {
            // A build-backend environment solved on behalf of the pixi
            // solve, one operation further down.
            let backend = progress.registry().allocate();
            let nested = backend
                .scope_active(async { queue_conda_solve(&progress) })
                .await;
            assert_ne!(progress.solve_bar(nested), Some(pixi_bar));
            CondaSolveReporter::on_started(&progress, nested);
            CondaSolveReporter::on_finished(&progress, nested);

            // A failure elsewhere clears the finished bars before the pixi
            // solve's own conda solve runs.
            progress.on_clear();

            let own = queue_conda_solve(&progress);
            CondaSolveReporter::on_started(&progress, own);
            assert_eq!(progress.solve_bar(own), Some(pixi_bar));
            CondaSolveReporter::on_finished(&progress, own);
        }));
        PixiSolveReporter::on_finished(&progress, pixi);
    }

    /// A pixi solve that fails before its conda solve, or whose conda solve
    /// is shared with another pixi solve, must still finish its bar.
    #[test]
    fn unclaimed_pixi_solve_bar_is_finished() {
        let progress = progress();
        let pixi = queue_pixi_solve(&progress);
        let pixi_bar = pixi_bar(&progress, pixi);
        PixiSolveReporter::on_finished(&progress, pixi);
        progress.on_clear();
        assert!(!progress.conda_solve_reporter.is_tracked(pixi_bar));
    }

    /// A conda solve cancelled while waiting for a solver never reports
    /// finishing, so it must not take the pixi solve's bar with it.
    #[test]
    fn cancelled_queued_conda_solve_leaves_the_bar_to_the_pixi_solve() {
        let progress = progress();
        let pixi = queue_pixi_solve(&progress);
        let pixi_bar = pixi_bar(&progress, pixi);
        block_on(pixi.scope_active(async { queue_conda_solve(&progress) }));
        PixiSolveReporter::on_finished(&progress, pixi);
        progress.on_clear();
        assert!(!progress.conda_solve_reporter.is_tracked(pixi_bar));
    }

    /// Every order in which the dispatcher can report a pixi solve, its own
    /// conda solve and a nested build-backend solve, with a clear after each
    /// event, must neither panic nor leave a bar behind.
    #[test]
    fn interleaved_solves_never_panic_and_finish_every_bar() {
        // 0-2: nested queued/started/finished, 3-5: own queued/started/
        // finished, 6: pixi solve finished. The nested solve may outlive the
        // pixi solve, as a shared backend environment does.
        let orders = (0..7u8).permutations(7).filter(|order| {
            let pos = |event| order.iter().position(|e| *e == event).unwrap();
            pos(0) < pos(1)
                && pos(1) < pos(2)
                && pos(3) < pos(4)
                && pos(4) < pos(5)
                && pos(5) < pos(6)
        });
        for order in orders {
            let progress = progress();
            let pixi = queue_pixi_solve(&progress);
            let backend = progress.registry().allocate();
            let (mut nested, mut own) = (None, None);
            for event in order {
                match event {
                    0 => {
                        nested = Some(block_on(pixi.scope_active(
                            backend.scope_active(async { queue_conda_solve(&progress) }),
                        )))
                    }
                    1 => CondaSolveReporter::on_started(&progress, nested.unwrap()),
                    2 => CondaSolveReporter::on_finished(&progress, nested.unwrap()),
                    3 => {
                        own = Some(block_on(
                            pixi.scope_active(async { queue_conda_solve(&progress) }),
                        ))
                    }
                    4 => CondaSolveReporter::on_started(&progress, own.unwrap()),
                    5 => CondaSolveReporter::on_finished(&progress, own.unwrap()),
                    _ => PixiSolveReporter::on_finished(&progress, pixi),
                }
                progress.on_clear();
            }
            assert!((0..3).all(|bar| !progress.conda_solve_reporter.is_tracked(bar)));
            assert!(progress.pixi_solve_bars.lock().is_empty());
            assert!(progress.solve_bars.lock().is_empty());
        }
    }
}
