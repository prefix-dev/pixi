//! Cycle detection + scoped cycle handling.
//!
//! This module is internal. It describes how cycles are detected,
//! how they are delivered, and how the pieces in
//! [`active_edges`] and [`guard`] compose. If you are looking for
//! the user-facing story, see the crate root.
//!
//! # The moving parts
//!
//! ```text
//!    ComputeCtx                 ActiveEdges                GuardStack
//!    ──────────                 ───────────                ──────────
//!    current: Option<AnyKey>    outgoing:                  frames:
//!    guard_stack: Arc<…>──┐     { caller -> {              [user guard, …]
//!                         │        target -> [             fallback:
//!                         │           {id, notify}, …      <task's oneshot>
//!                         │        ] } }                   parent:
//!                         └──────────────►innermost()      <outer stack>
//! ```
//!
//! - Each running compute task has a [`ComputeCtx`][ctx] carrying
//!   the key being computed (`current`) and a pointer to its
//!   [`GuardStack`]. Parallel sub-ctxes use [`new_branch`][br] so
//!   their pushes stay local but outer guards (and the task
//!   fallback at the root) remain visible via the `parent` chain.
//! - [`ActiveEdges`] is the engine-wide outstanding-wait graph.
//!   Every live `ctx.compute(&dep)` has an edge `caller -> dep`
//!   in it, tagged with a notify target and a unique [`EdgeId`].
//! - Each edge's notify target is the [`GuardHandle`] returned by
//!   [`GuardStack::innermost`] at edge-creation time: a user
//!   `with_cycle_guard` frame if one is open on this branch, else
//!   the task's synthetic fallback.
//!
//! [ctx]: crate::ComputeCtx
//! [br]: guard::GuardStack::new_branch
//! [`ActiveEdges`]: active_edges::ActiveEdges
//! [`EdgeId`]: active_edges::EdgeId
//! [`GuardHandle`]: guard::GuardHandle
//! [`GuardStack`]: guard::GuardStack
//! [`GuardStack::innermost`]: guard::GuardStack::innermost
//!
//! # Detection: forward BFS from target
//!
//! When a compute body calls `ctx.compute(&target)`, the context
//! resolves the notify target from its guard stack and calls
//! [`ActiveEdges::try_add(caller, target, notify)`][try_add]. That
//! call takes the single [`ActiveEdges`] mutex, BFS-walks forward
//! from `target` along existing edges, and either installs the new
//! edge or reports a cycle. The lock is held across the whole
//! check-and-insert so two concurrent [`try_add`][try_add]s cannot
//! each install one half of a cycle.
//!
//! ```text
//!                    (would-be new edge: caller -> target)
//!     caller   ──?──▶   target ──▶ X ──▶ Y ──▶ caller
//!                                                 ▲
//!                                                 │
//!                                           BFS reaches caller
//!                                           via existing edges
//!                                           ═══▶ cycle detected
//! ```
//!
//! [try_add]: active_edges::ActiveEdges::try_add
//!
//! On cycle, [`try_add`][try_add] returns a [`DetectedCycle`] with
//! two pieces:
//!
//! - `path` — the distinct keys on the cycle in order,
//!   `[caller, target, …]`, rebuilt from the BFS's parent-edge map.
//!   The closing edge is from the last entry back to the first.
//!   Used to render [`CycleError`] for the user.
//! - `targets` — one [`GuardHandle`] per edge in the ring, starting
//!   with the would-be closing edge's target (the `notify` the
//!   caller passed in) and then every live record on each pair the
//!   BFS traversed. When a `(u, v)` pair has multiple records
//!   (parallel siblings awaiting the same dep), every record's
//!   notify target is in the list.
//!
//! [`DetectedCycle`]: active_edges::DetectedCycle
//! [`CycleError`]: guard::CycleError
//!
//! # Delivery: notify the edge-captured targets
//!
//! [`ComputeCtx::compute`][cc] iterates `detected.targets`, dedups
//! them by `Arc::as_ptr` (a scope may own multiple edges in the
//! ring), and calls [`notify`][notify] on each. Notify is a
//! take-once oneshot send; the first call arms the receiver, later
//! calls are no-ops.
//!
//! The detector does not consult any mutable stack at detection
//! time. That is what makes scope routing correct:
//!
//! - A user [`with_cycle_guard`][wc] scope fires only if one of its
//!   edges is on the cycle. Siblings in other parallel branches or
//!   in other tasks are never affected by a scope that did not
//!   itself install a cycle-path edge.
//! - A task's fallback fires only if one of its edges on the cycle
//!   was installed with no user scope open. That ends the task
//!   with [`Err(ComputeError::Cycle)`][ec], which rides up
//!   `shared.await` to the root.
//!
//! [cc]: crate::ComputeCtx::compute
//! [notify]: guard::GuardHandle::notify
//! [wc]: crate::ComputeCtx::with_cycle_guard
//! [ec]: crate::ComputeError::Cycle
//!
//! # Walkthrough 1: unguarded two-key cycle
//!
//! ```text
//!   Root(A) ─────────▶  A (spawned, fallback_A)
//!                       │  A.compute() calls ctx.compute(&B)
//!                       ▼
//!                  ActiveEdges: { A -> B : fallback_A }
//!                       │
//!                       ▼
//!                       B (spawned, fallback_B)
//!                          B.compute() calls ctx.compute(&A)
//!                          try_add(B, A, fallback_B):
//!                              BFS from A reaches B  ═══▶  cycle!
//!
//!   detected.path    = [B, A, B]
//!   detected.targets = [fallback_B, fallback_A]
//! ```
//!
//! Both fallbacks fire. A's and B's tasks each take their outer
//! `select!`'s cycle arm and return `Err(Cycle)`. `Root` awaits
//! A's shared, sees `Err(Cycle)`, and returns it to the user.
//!
//! # Walkthrough 2: parallel siblings on the same dep
//!
//! ```text
//!    Root ──compute2─┐─▶ branch A:
//!                    │     with_cycle_guard(guard_A) {
//!                    │       ctx.compute(&Shared)
//!                    │     }
//!                    │
//!                    └─▶ branch B:
//!                          with_cycle_guard(guard_B) {
//!                            ctx.compute(&Shared)
//!                          }
//!
//!   ActiveEdges (multigraph): {
//!     Root -> Shared : [ {id1, guard_A},
//!                        {id2, guard_B} ]
//!   }
//!
//!   Shared.compute() calls ctx.compute(&Root).
//!   try_add(Shared, Root, fb_Shared):
//!       BFS from Root reaches Shared, via BOTH records.
//!
//!   detected.path    = [Shared, Root, Shared]
//!   detected.targets = [fb_Shared, guard_A, guard_B]
//! ```
//!
//! `guard_A` fires → branch A's `with_cycle_guard` returns
//! `Err(CycleError)`; similarly for branch B. Each branch recovers
//! in its own scope. Root's `compute2` joins their values.
//!
//! If the graph had stored a single notify target per
//! `(caller, target)` pair, one branch would have been
//! overwritten by the other, and only one branch would have been
//! notified on cycle; the overwritten branch would also have had
//! its still-live edge removed the moment the "winning" branch
//! exited. See [`active_edges`] for the multigraph details.
//!
//! # Walkthrough 3: strict scope (guard above the cycle)
//!
//! ```text
//!    Outer (with_cycle_guard scope around ctx.compute(&Inner))
//!      └──▶ Inner(A) ──▶ Inner(B) ──▶ Inner(A)
//!
//!   Edges when the cycle closes:
//!     Outer    -> Inner(A) : guard_Outer    (captured in Outer's scope)
//!     Inner(A) -> Inner(B) : fallback_A     (no user scope on A)
//!     Inner(B) -> Inner(A) : fallback_B     (the closing edge)
//!
//!   detected.targets = [fallback_B, fallback_A]
//! ```
//!
//! `guard_Outer` is NOT in the targets: none of the edges on the
//! cycle was installed while it was the innermost scope. Outer is
//! above the cycle, not on it. Both `Inner` fallbacks fire, both
//! `Inner` tasks end with `Err(Cycle)`, Outer's awaited
//! `ctx.compute(&Inner(A))` returns that error, and Outer's own
//! `ctx.compute` routes the transitive error straight to Outer's
//! fallback (bypassing any open user scope, see
//! [`GuardStack::fallback`][fb]). The cycle surfaces at
//! [`ComputeEngine::compute`][ecc] as `Err(Cycle)`.
//!
//! [fb]: guard::GuardStack::fallback
//! [ecc]: crate::ComputeEngine::compute
//!
//! # Edge removal: per-record
//!
//! When a caller's `ctx.compute` future drops (shared resolved, or
//! the caller itself was dropped), the `EdgeGuard` it holds calls
//! [`ActiveEdges::remove(caller, target, id)`][remove]. Removal is
//! identified by [`EdgeId`] so parallel siblings on the same
//! `(caller, target)` pair are independent: one branch exiting
//! cannot yank the other branch's still-live wait.
//!
//! [remove]: active_edges::ActiveEdges::remove
//!
//! # Concurrency and cycle identity
//!
//! Two consequences of the "fire whatever edges are on the ring at
//! detection time" rule are worth calling out:
//!
//! **Cycle paths are not canonical.** When multiple distinct
//! cycles race to close at nearly the same moment, each successful
//! detection reports one specific ring it observed and fires the
//! targets on that ring. Each [`GuardHandle`] is take-once, so the
//! first notify against a given scope wins and later notifies from
//! other cycles are silently dropped. Callers that render or
//! compare the reported [`CycleError::path`] should treat it as
//! "one detected cycle through this key", not as a stable
//! canonical representation of the graph's cyclic structure.
//!
//! [`CycleError::path`]: guard::CycleError::path
//!
//! **Sibling awaits may race with detection.** Under a
//! single-threaded scheduler, `futures::future::join` polls sibling
//! branches in sequence before any spawned sub-task gets to run, so
//! two branches requesting the same dep in the same compute frame
//! both have their edges installed before the dep's task can close
//! a cycle. Under a multi-threaded scheduler, the spawned dep can
//! run on a separate worker and close the cycle before the second
//! sibling has polled. In that case the second sibling sees the
//! cycle transitively via `shared.await -> Err(Cycle)` and routes
//! to the task fallback (strict semantics: transitive cycles bypass
//! user scopes on the awaiter), rather than being notified through
//! its own `with_cycle_guard`. The cycle still surfaces to the
//! user (the root gets `Err(Cycle)` either way); only the question
//! of "which scope catches" depends on timing.

pub(crate) mod active_edges;
pub(crate) mod guard;

pub use guard::CycleError;
