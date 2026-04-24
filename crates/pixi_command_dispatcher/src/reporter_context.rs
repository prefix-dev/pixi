//! Task-local carrying the [`ReporterContext`] of the currently-running
//! task, for attribution of child operations.
//!
//! Pattern is Buck2's `EventDispatcher` via `tokio::task_local!`: when a
//! processor task handler starts, it wraps its work with
//! [`CURRENT_REPORTER_CONTEXT`]`.scope(Some(own_context), ..)`. Anything
//! that runs inside then sees that context, including a
//! [`ComputeEngine`](pixi_compute_engine::ComputeEngine) call whose
//! freshly spawned task inherits the task-local via the engine's
//! [`SpawnHook`](pixi_compute_engine::SpawnHook) (registered by the
//! [`CommandDispatcherBuilder`](crate::CommandDispatcherBuilder)).
//!
//! Child operations (e.g. a git checkout triggered by a source metadata
//! task) read this task-local via [`current_reporter_context`] to
//! populate the `reason` argument of their reporter callbacks.

use crate::reporter::ReporterContext;

tokio::task_local! {
    /// The [`ReporterContext`] of the task currently executing on this
    /// tokio task. `None` when no parent task has installed a scope.
    pub(crate) static CURRENT_REPORTER_CONTEXT: Option<ReporterContext>;
}

/// Read the current task's [`ReporterContext`], if one has been
/// installed via [`CURRENT_REPORTER_CONTEXT::scope`].
///
/// Returns `None` when no scope is active on the current task.
pub(crate) fn current_reporter_context() -> Option<ReporterContext> {
    CURRENT_REPORTER_CONTEXT.try_get().ok().flatten()
}
