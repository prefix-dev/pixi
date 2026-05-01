//! `ctx.solve_conda` extension trait. Runs a conda solve on the
//! blocking-task pool, subject to the `max_concurrent_solves` limit
//! enforced by the command dispatcher's semaphore, and drives the
//! [`CondaSolveReporter`] lifecycle (`on_queued` → `on_started` →
//! `on_finished`).
//!
//! The ext method takes an existing [`SolveCondaEnvironmentSpec`] so
//! it slots into the current reporter contract without any translation
//! layer. It does **not** fetch repodata; callers pre-fetch and pass
//! it via `spec.binary_repodata`, matching the existing
//! `SolveCondaEnvironmentSpec` solver contract.

use std::sync::Arc;

use pixi_compute_engine::{ComputeCtx, DataStore};
use pixi_record::PixiRecord;

use crate::SolveCondaEnvironmentSpec;
use crate::compute_data::{HasCondaSolveSemaphore, HasReporter};
use crate::injected_config::ChannelConfigKey;
use crate::reporter::{CondaSolveId, CondaSolveReporter, Reporter, ReporterContext};
use crate::reporter_context::current_reporter_context;
use crate::reporter_lifecycle::{Active, LifecycleKind, ReporterLifecycle};
use crate::solve_conda::{SolveCondaBlockingError, SolveCondaEnvironmentError};

/// `LifecycleKind` for conda solves.
struct CondaSolveReporterLifecycle;

impl LifecycleKind for CondaSolveReporterLifecycle {
    type Reporter<'r> = dyn CondaSolveReporter + 'r;
    type Id = CondaSolveId;
    type Env = SolveCondaEnvironmentSpec;

    fn queue<'r>(
        reporter: Option<&'r dyn Reporter>,
        parent: Option<ReporterContext>,
        env: &Self::Env,
    ) -> Option<Active<'r, Self::Reporter<'r>, Self::Id>> {
        reporter
            .and_then(|r| r.as_conda_solve_reporter())
            .map(|r| Active {
                reporter: r,
                id: r.on_queued(parent, env),
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
        let reporter = data.reporter().map(Arc::as_ref);

        let lifecycle = ReporterLifecycle::<CondaSolveReporterLifecycle>::queued(
            reporter,
            current_reporter_context(),
            &spec,
        );

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
