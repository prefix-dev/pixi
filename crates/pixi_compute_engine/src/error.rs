//! Framework-level errors returned by
//! [`ComputeEngine::compute`](crate::ComputeEngine::compute).
//!
//! User-level errors live inside [`Key::Value`](crate::Key::Value);
//! this enum carries only the framework's own failure modes.
//!
//! # Cycles
//!
//! A detected cycle is first offered to every
//! [`ComputeCtx::with_cycle_guard`](crate::ComputeCtx::with_cycle_guard)
//! scope that sits on the cycle path, as a
//! [`CycleError`]. If no user guard catches, the
//! cycle surfaces at the engine boundary as [`ComputeError::Cycle`],
//! carrying the full ring of keys.

use crate::CycleError;

/// An error returned by
/// [`ComputeEngine::compute`](crate::ComputeEngine::compute).
///
/// This enum carries only *framework*-level failure modes that remain
/// meaningful at the engine boundary. A Key's own compute body calls
/// [`ComputeCtx::compute`](crate::ComputeCtx::compute), which returns
/// the child's [`Value`](crate::Key::Value) directly (no `Result`).
/// User-level failures live inside that `Value`.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ComputeError {
    /// The underlying spawned task was aborted before it could produce
    /// a value.
    ///
    /// This happens when every subscriber to an in-flight compute
    /// drops before the compute finishes: the engine cancels the task
    /// because nobody is waiting for the value. A request that
    /// re-arrives later will spawn a fresh compute.
    #[error("compute was canceled")]
    Canceled,

    /// A dependency cycle was detected that no
    /// [`ComputeCtx::with_cycle_guard`](crate::ComputeCtx::with_cycle_guard)
    /// scope caught.
    ///
    /// The wrapped [`CycleError`]'s `path` lists the distinct keys
    /// on the cycle in order: `[caller, target, ...]`, where the
    /// closing edge is from the last entry back to the first.
    #[error("compute cycle detected: {0}")]
    Cycle(CycleError),
}
