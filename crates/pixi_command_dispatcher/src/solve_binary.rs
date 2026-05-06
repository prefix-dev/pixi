//! `ctx.solve_conda` extension trait. Runs a conda solve on the
//! blocking-task pool, subject to the `max_concurrent_solves` limit
//! enforced by the command dispatcher's semaphore. Callers pre-fetch
//! repodata and pass it via `spec.binary_repodata`.

use pixi_compute_engine::{ComputeCtx, DataStore};
use pixi_record::PixiRecord;

use crate::SolveCondaEnvironmentSpec;
use crate::compute_data::{HasCondaSolveReporter, HasCondaSolveSemaphore};
use crate::injected_config::ChannelConfigKey;
use crate::reporter::CondaSolveReporter;
use crate::reporter_lifecycle::{Active, LifecycleKind, ReporterLifecycle};
use crate::solve_conda::{SolveCondaBlockingError, SolveCondaEnvironmentError};
use pixi_compute_reporters::OperationId;

/// `LifecycleKind` for conda solves.
struct CondaSolveReporterLifecycle;

impl LifecycleKind for CondaSolveReporterLifecycle {
    type Reporter<'r> = dyn CondaSolveReporter + 'r;
    type Id = OperationId;
    type Env = SolveCondaEnvironmentSpec;

    fn queue<'r>(
        reporter: Option<&'r Self::Reporter<'r>>,
        env: &Self::Env,
    ) -> Option<Active<'r, Self::Reporter<'r>, Self::Id>> {
        reporter.map(|r| Active {
            reporter: r,
            id: r.on_queued(env),
        })
    }

    fn on_started<'r>(active: &Active<'r, Self::Reporter<'r>, Self::Id>) {
        active.reporter.on_started(active.id);
    }

    fn on_finished<'r>(active: Active<'r, Self::Reporter<'r>, Self::Id>) {
        active.reporter.on_finished(active.id);
    }
}

/// Extension trait on [`ComputeCtx`] that runs a conda solve with
/// concurrency limiting and progress reporting.
pub trait SolveCondaExt {
    /// Reports progress via `Arc<dyn CondaSolveReporter>` set on the engine `DataStore`, if any.
    fn solve_conda(
        &mut self,
        spec: SolveCondaEnvironmentSpec,
    ) -> impl Future<Output = Result<Vec<PixiRecord>, SolveCondaEnvironmentError>> + Send;
}

impl SolveCondaExt for ComputeCtx {
    async fn solve_conda(
        &mut self,
        spec: SolveCondaEnvironmentSpec,
    ) -> Result<Vec<PixiRecord>, SolveCondaEnvironmentError> {
        let channel_config = self.compute(&ChannelConfigKey).await;
        let data: &DataStore = self.global_data();
        let semaphore = data.conda_solve_semaphore().cloned();
        let reporter = data.conda_solve_reporter().cloned();

        let lifecycle =
            ReporterLifecycle::<CondaSolveReporterLifecycle>::queued(reporter.as_deref(), &spec);

        let _permit = match semaphore.as_ref() {
            Some(s) => Some(
                s.acquire()
                    .await
                    .expect("conda solve semaphore is never closed"),
            ),
            None => None,
        };
        let _lifecycle = lifecycle.start();

        match spec.solve_on_blocking_pool(channel_config).await {
            Ok(records) => Ok(records),
            Err(SolveCondaBlockingError::Solve(e)) => Err(e),
            Err(SolveCondaBlockingError::Panic(p)) => std::panic::resume_unwind(p),
            Err(SolveCondaBlockingError::JoinCancelled) => {
                // Runtime shutdown; the compute body's future is about
                // to be dropped anyway. Return the solve-equivalent for
                // shape, but the caller is unlikely to observe this.
                Err(SolveCondaEnvironmentError::SolveError(
                    rattler_solve::SolveError::Cancelled,
                ))
            }
        }
    }
}
