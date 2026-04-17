//! Scoped cycle guards. A compute body can wrap a sub-scope in
//! [`ComputeCtx::with_cycle_guard`](crate::ComputeCtx::with_cycle_guard)
//! to opt into recovering from cycles: the guard races its inner future
//! against a notification channel that fires when a cycle is detected
//! beneath the scope. Outside any guard, the task's synthetic fallback
//! fires instead and the cycle surfaces at
//! [`ComputeEngine::compute`](crate::ComputeEngine::compute) as
//! [`Err(ComputeError::Cycle)`](crate::ComputeError::Cycle).

use std::{fmt, sync::Arc};

use parking_lot::Mutex;
use tokio::sync::oneshot;

use crate::AnyKey;

/// A detected dependency cycle, delivered to the nearest enclosing
/// [`ComputeCtx::with_cycle_guard`](crate::ComputeCtx::with_cycle_guard).
///
/// The [`path`](Self::path) forms the ring of keys that closed the cycle:
/// `[caller, target, ..., caller]`, where `caller` and `target` are the
/// endpoints of the edge that closed the loop.
#[derive(Clone, Debug)]
pub struct CycleError {
    /// The keys on the cycle, starting and ending with the closing key.
    pub path: Vec<AnyKey>,
}

impl fmt::Display for CycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for key in &self.path {
            if !first {
                write!(f, " -> ")?;
            }
            write!(f, "{key}")?;
            first = false;
        }
        Ok(())
    }
}

/// Per-guard bookkeeping shared between a [`ComputeCtx`](crate::ComputeCtx)
/// guard frame and the [`ComputeCtx::with_cycle_guard`](crate::ComputeCtx::with_cycle_guard)
/// caller's `select!` arm.
///
/// The caller holds the [`oneshot::Receiver`]; the ctx's guard stack
/// holds a `GuardHandle` wrapping the [`oneshot::Sender`]. When a cycle
/// is detected, the detector pulls the handle off the stack and fires
/// the sender, which causes the `select!` to take the cycle branch.
pub(crate) struct GuardHandle {
    sender: Mutex<Option<oneshot::Sender<CycleError>>>,
}

impl fmt::Debug for GuardHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let armed = self.sender.lock().is_some();
        f.debug_struct("GuardHandle")
            .field("armed", &armed)
            .finish()
    }
}

impl GuardHandle {
    pub(crate) fn new(sender: oneshot::Sender<CycleError>) -> Self {
        Self {
            sender: Mutex::new(Some(sender)),
        }
    }

    /// Fire the cycle notification. No-op if the guard was already
    /// notified or if the caller's `select!` has already resolved.
    pub(crate) fn notify(&self, err: CycleError) {
        if let Some(sender) = self.sender.lock().take() {
            let _ = sender.send(err);
        }
    }
}

/// The stack of active cycle guards carried on a [`ComputeCtx`](crate::ComputeCtx).
///
/// Shared between all sub-ctxes in the same compute frame (via
/// [`Arc`]) so that a guard installed in a parent scope remains
/// visible to child `ctx.compute(...)` calls made inside parallel
/// combinators.
///
/// # Two layers: user frames and the synthetic fallback
///
/// `frames` contains only user [`with_cycle_guard`](crate::ComputeCtx::with_cycle_guard)
/// scopes. `fallback` holds a single synthetic [`GuardHandle`]
/// installed by [`spawn_compute_future`](crate::ctx) when the task
/// is created; it ends the task with
/// `Err(ComputeError::Cycle(..))` when fired.
///
/// The two layers are kept separate so that the detector and the
/// transitive-propagation path can route differently:
///
/// - The detector (on a cycle-path key) calls
///   [`innermost`](Self::innermost), which prefers the user's
///   innermost `with_cycle_guard` and falls back to the synthetic
///   frame if no user scope is open.
/// - [`ComputeCtx::compute`](crate::ComputeCtx::compute)'s
///   `Err(Cycle)` path (when an awaited dependency failed due to a
///   cycle that did not involve this key) calls
///   [`fallback`](Self::fallback), bypassing user frames entirely.
///   This enforces that a `with_cycle_guard` only catches cycles
///   its own key participates in.
///
/// # Semantics: innermost wins within a task
///
/// Nested `with_cycle_guard` calls do not implement scope-depth
/// containment. The innermost user frame on each cycle-path task
/// is the one notified by the detector. Nested guards are useful
/// for try-then-fallback within the same task, not for routing
/// cycles to different handlers based on where they originated.
///
/// # Parallel branches have their own stacks
///
/// Parallel sub-ctxes (see [`ComputeCtx::compute2`](crate::ComputeCtx::compute2)
/// and friends) each get a fresh branch stack that chains to the
/// enclosing compute body's stack via [`parent`](Self::parent).
/// [`push`](Self::push) and [`remove`](Self::remove) operate only
/// on the branch's own frames, so a sibling branch's scope exit
/// cannot disturb another branch. [`innermost`](Self::innermost)
/// and [`fallback`](Self::fallback) walk the parent chain, so
/// outer guards installed before the parallel split (and the
/// task's synthetic fallback at the root) remain reachable from
/// every branch.
///
/// Cross-task notification does not consult this structure at
/// detection time. Each active edge in
/// [`ActiveEdges`](crate::cycle::active_edges::ActiveEdges)
/// captures its notify target at edge-creation time (resolved
/// from the caller's branch-local stack via
/// [`innermost`](Self::innermost)), so the detector simply fires
/// the targets carried by the cycle's edges. Scopes on other
/// tasks are reachable through their own tasks' captured edges,
/// not through any shared registry.
#[derive(Default)]
pub(crate) struct GuardStack {
    frames: Mutex<Vec<Arc<GuardHandle>>>,
    /// Only set on the task's root stack (in `spawn_compute_future`).
    /// Branch stacks have `None` here and defer to `parent` on
    /// [`fallback`](Self::fallback) lookups.
    fallback: Mutex<Option<Arc<GuardHandle>>>,
    /// Set on branch stacks to the enclosing compute body's stack.
    /// `None` on a task's root stack.
    parent: Option<Arc<GuardStack>>,
}

impl GuardStack {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Create a branch stack that chains to `parent` for guard
    /// lookups. Used by parallel combinators so each sub-ctx has
    /// its own local frames.
    pub(crate) fn new_branch(parent: Arc<GuardStack>) -> Self {
        Self {
            frames: Mutex::new(Vec::new()),
            fallback: Mutex::new(None),
            parent: Some(parent),
        }
    }

    pub(crate) fn push(&self, handle: Arc<GuardHandle>) {
        self.frames.lock().push(handle);
    }

    /// Remove the specific frame `handle` from the stack on scope
    /// exit, identified by [`Arc::ptr_eq`].
    ///
    /// Identity-based removal is load-bearing when parallel branches
    /// share this stack: a plain "pop the top" would let one
    /// branch's scope exit remove a different, still-active
    /// branch's frame. With concurrent
    /// [`ComputeCtx::with_cycle_guard`](crate::ComputeCtx::with_cycle_guard)
    /// calls in different branches, the branch that opened a frame
    /// first may close last (or vice-versa), so the frame to remove
    /// is not necessarily the top.
    ///
    /// Searches from the back because scope exits are LIFO in the
    /// common (single-branch) case, so the target frame is almost
    /// always at or near the top. No-op if the handle is not in the
    /// stack.
    pub(crate) fn remove(&self, handle: &Arc<GuardHandle>) {
        let mut frames = self.frames.lock();
        if let Some(pos) = frames.iter().rposition(|h| Arc::ptr_eq(h, handle)) {
            frames.remove(pos);
        }
    }

    /// Install the task's synthetic fallback. Called once by
    /// `spawn_compute_future` before user code runs.
    pub(crate) fn set_fallback(&self, handle: Arc<GuardHandle>) {
        *self.fallback.lock() = Some(handle);
    }

    /// The detector's notify target for a cycle-path key: the
    /// innermost user frame visible from this stack (walking the
    /// parent chain), falling back to the task's synthetic
    /// fallback if no user frame is open anywhere up the chain.
    pub(crate) fn innermost(&self) -> Option<Arc<GuardHandle>> {
        if let Some(user) = self.frames.lock().last().cloned() {
            return Some(user);
        }
        if let Some(parent) = &self.parent {
            return parent.innermost();
        }
        self.fallback.lock().clone()
    }

    /// The task's synthetic fallback, ignoring every user frame.
    /// Used by the transitive propagation path in
    /// [`ComputeCtx::compute`](crate::ComputeCtx::compute) so a
    /// user guard only catches cycles its own key is on. Walks the
    /// parent chain to reach the root stack, where the fallback
    /// actually lives.
    pub(crate) fn fallback(&self) -> Option<Arc<GuardHandle>> {
        if let Some(f) = self.fallback.lock().clone() {
            return Some(f);
        }
        if let Some(parent) = &self.parent {
            return parent.fallback();
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle() -> Arc<GuardHandle> {
        let (tx, _rx) = oneshot::channel();
        Arc::new(GuardHandle::new(tx))
    }

    /// Removing a frame that was pushed before another must leave
    /// the later frame intact. Regression test for a bug where
    /// `pop()` blindly removed the top frame, so a parallel
    /// branch exiting first would delete a sibling branch's
    /// still-active guard.
    #[test]
    fn remove_by_identity_preserves_later_frames() {
        let stack = GuardStack::new();
        let first = handle();
        let second = handle();
        stack.push(first.clone());
        stack.push(second.clone());

        // Remove the non-top frame; the top one must survive.
        stack.remove(&first);

        let innermost = stack.innermost().expect("second should remain");
        assert!(
            Arc::ptr_eq(&innermost, &second),
            "innermost should still be the later-pushed frame",
        );
    }

    #[test]
    fn remove_unknown_handle_is_a_noop() {
        let stack = GuardStack::new();
        let on_stack = handle();
        let not_on_stack = handle();
        stack.push(on_stack.clone());

        stack.remove(&not_on_stack);

        let innermost = stack.innermost().expect("on_stack should remain");
        assert!(Arc::ptr_eq(&innermost, &on_stack));
    }
}
