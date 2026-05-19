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
use crate::solve_conda::{SolveCondaBlockingError, SolveCondaEnvironmentError};
use pixi_compute_reporters::{Active, LifecycleKind, OperationId, ReporterLifecycle};

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
        let fn_started = std::time::Instant::now();
        let config_started = std::time::Instant::now();
        let channel_config = self.compute(&ChannelConfigKey).await;
        let channel_config_elapsed_ms = config_started.elapsed().as_millis() as u64;
        let data: &DataStore = self.global_data();
        let semaphore = data.conda_solve_semaphore().cloned();
        let reporter = data.conda_solve_reporter().cloned();

        let lifecycle =
            ReporterLifecycle::<CondaSolveReporterLifecycle>::queued(reporter.as_deref(), &spec);

        // Time the semaphore wait separately from the actual solve so we
        // can tell whether a slow solve is genuinely CPU-bound work or just
        // queued behind other solves holding the slot.
        let acquire_started = std::time::Instant::now();
        let _permit = match semaphore.as_ref() {
            Some(s) => Some(
                s.acquire()
                    .await
                    .expect("conda solve semaphore is never closed"),
            ),
            None => None,
        };
        let acquire_elapsed_ms = acquire_started.elapsed().as_millis() as u64;
        tracing::debug!(
            channel_config_elapsed_ms,
            acquire_elapsed_ms,
            permit = semaphore.is_some(),
            "conda solve semaphore acquired"
        );
        let _lifecycle = lifecycle.start();

        let solve_started = std::time::Instant::now();
        let result = spec.solve_on_blocking_pool(channel_config).await;
        let solve_elapsed_ms = solve_started.elapsed().as_millis() as u64;
        let total_elapsed_ms = fn_started.elapsed().as_millis() as u64;
        let unaccounted_ms = total_elapsed_ms
            .saturating_sub(channel_config_elapsed_ms + acquire_elapsed_ms + solve_elapsed_ms);
        tracing::debug!(
            channel_config_elapsed_ms,
            acquire_elapsed_ms,
            solve_elapsed_ms,
            total_elapsed_ms,
            unaccounted_ms,
            "solve_on_blocking_pool returned"
        );

        match result {
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
