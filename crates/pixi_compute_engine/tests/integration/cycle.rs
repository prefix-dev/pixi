//! Cycle detection tests.
//!
//! Cycles are detected synchronously inside `ctx.compute(...)` via a
//! global active-edge graph. Each edge captures the notify target
//! that was in scope when it was installed (the caller's innermost
//! [`with_cycle_guard`](pixi_compute_engine::ComputeCtx::with_cycle_guard)
//! or the task's synthetic fallback), and on detection every target
//! in the cycle ring is notified. If no user scope on the ring
//! catches, the task fallbacks fire and the cycle surfaces at
//! [`ComputeEngine::compute`](pixi_compute_engine::ComputeEngine::compute)
//! as `Err(ComputeError::Cycle(..))`.

use std::sync::Arc;

use derive_more::Display;
use futures::FutureExt;
use pixi_compute_engine::{ComputeCtx, ComputeEngine, ComputeError, Key};
use tokio::sync::Notify;

/// Which cycle shape the `CycleKey` family should form during a test.
#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
enum CyclePattern {
    /// `A -> A`
    SelfLoop,
    /// `A -> B -> C -> B`
    Deep,
}

/// A Key whose `compute` wraps its dependency request in
/// `with_cycle_guard`, so a cycle involving this key is caught at
/// this frame's guard and folded into the Value.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{name}")]
struct CycleKey {
    name: char,
    pattern: CyclePattern,
}
impl Key for CycleKey {
    type Value = Result<(), String>;
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let next = match (self.pattern, self.name) {
            (CyclePattern::SelfLoop, 'A') => Some('A'),
            (CyclePattern::Deep, 'A') => Some('B'),
            (CyclePattern::Deep, 'B') => Some('C'),
            (CyclePattern::Deep, 'C') => Some('B'),
            _ => None,
        };
        let Some(n) = next else {
            return Ok(());
        };
        let child = CycleKey {
            name: n,
            pattern: self.pattern,
        };
        // Propagate the child's Value up: if the child's own guard
        // caught a cycle, its Err comes back as `inner`. If OUR
        // guard caught the cycle, we fold it ourselves.
        let child_result = ctx
            .with_cycle_guard(move |ctx| async move { ctx.compute(&child).await }.boxed())
            .await;
        match child_result {
            Ok(inner) => inner,
            Err(cycle) => Err(format!("cycle:{cycle}")),
        }
    }
}

/// Render whatever error surfaces when a `CycleKey` chain closes a
/// cycle, caught at the first guard that wins its select.
async fn render_cycle(pattern: CyclePattern) -> String {
    let engine = ComputeEngine::new();
    engine
        .compute(&CycleKey { name: 'A', pattern })
        .await
        .unwrap()
        .unwrap_err()
}

/// `A -> A`: the self-loop fires the guard on A's own compute body.
#[tokio::test(flavor = "current_thread")]
async fn direct_cycle() {
    let msg = render_cycle(CyclePattern::SelfLoop).await;
    assert!(msg.starts_with("cycle:"), "unexpected rendering: {msg}");
    assert!(msg.contains("CycleKey(A)"), "rendering missing A: {msg}");
}

/// `A -> B -> C -> B`: closes on B, at least one of the guards on the
/// cycle path catches it.
#[tokio::test(flavor = "current_thread")]
async fn deep_cycle() {
    let msg = render_cycle(CyclePattern::Deep).await;
    assert!(msg.starts_with("cycle:"));
}

/// A Key that cycles but installs no guard. Detection must surface
/// as `Err(ComputeError::Cycle(..))` at the engine boundary, carrying
/// the full cycle path.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{_0}")]
struct UnguardedCycleKey(char);
impl Key for UnguardedCycleKey {
    type Value = ();
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let next = match self.0 {
            'A' => 'B',
            'B' => 'A',
            _ => return,
        };
        ctx.compute(&UnguardedCycleKey(next)).await;
    }
}

/// A pure self-loop at the engine boundary: a Key whose compute
/// immediately requests itself. Pins the `ActiveEdges::try_add`
/// caller-equals-target early-return path through the public API.
#[tokio::test(flavor = "current_thread")]
async fn unguarded_self_loop_returns_cycle_error() {
    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("SelfLoop")]
    struct SelfLoop;
    impl Key for SelfLoop {
        type Value = ();
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            ctx.compute(&SelfLoop).await;
        }
    }

    let engine = ComputeEngine::new();
    let err = engine.compute(&SelfLoop).await.unwrap_err();
    let ComputeError::Cycle(cycle) = err else {
        panic!("expected ComputeError::Cycle, got: {err:?}");
    };
    // Ring for a self-loop is [K, K].
    assert_eq!(cycle.path.len(), 2);
    assert_eq!(cycle.path.first(), cycle.path.last());
}

/// Without any `with_cycle_guard` on the cycle path, a detected
/// cycle is delivered to the caller of `ComputeEngine::compute` as
/// `Err(ComputeError::Cycle(..))` with the full path.
#[tokio::test(flavor = "current_thread")]
async fn unguarded_cycle_returns_cycle_error() {
    let engine = ComputeEngine::new();
    let err = engine.compute(&UnguardedCycleKey('A')).await.unwrap_err();
    let ComputeError::Cycle(cycle) = err else {
        panic!("expected ComputeError::Cycle, got: {err:?}");
    };
    let rendered = format!("{cycle}");
    assert!(
        rendered.contains("UnguardedCycleKey(A)"),
        "cycle path should mention A: {rendered}",
    );
    assert!(
        rendered.contains("UnguardedCycleKey(B)"),
        "cycle path should mention B: {rendered}",
    );
    // Ring form: first and last key match.
    assert!(cycle.path.len() >= 3);
    assert_eq!(cycle.path.first(), cycle.path.last());
}

/// Two concurrent root computes in a cross-root cycle, neither
/// installing a user guard. With strict semantics each task's own
/// synthetic fallback is what fires on the cycle-path keys, and
/// both `ComputeEngine::compute` calls return `Err(Cycle(..))`
/// independently.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unguarded_cross_root_cycle_surfaces_to_both_roots() {
    use pixi_compute_engine::DataStore;

    struct Handshake {
        a_started: Notify,
        b_started: Notify,
    }
    trait HasHandshake {
        fn handshake(&self) -> &Handshake;
    }
    impl HasHandshake for DataStore {
        fn handshake(&self) -> &Handshake {
            self.get::<Arc<Handshake>>().as_ref()
        }
    }

    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("A")]
    struct KeyA;
    impl Key for KeyA {
        type Value = ();
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            ctx.global_data().handshake().a_started.notify_one();
            ctx.global_data().handshake().b_started.notified().await;
            ctx.compute(&KeyB).await;
        }
    }

    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("B")]
    struct KeyB;
    impl Key for KeyB {
        type Value = ();
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            ctx.global_data().handshake().a_started.notified().await;
            ctx.global_data().handshake().b_started.notify_one();
            ctx.compute(&KeyA).await;
        }
    }

    let handshake = Arc::new(Handshake {
        a_started: Notify::new(),
        b_started: Notify::new(),
    });
    let engine = ComputeEngine::builder().with_data(handshake).build();
    let e2 = engine.clone();
    let a_task = tokio::spawn(async move { engine.compute(&KeyA).await });
    let b_task = tokio::spawn(async move { e2.compute(&KeyB).await });
    let (a_res, b_res) = tokio::join!(a_task, b_task);

    let a_err = a_res.unwrap().unwrap_err();
    let b_err = b_res.unwrap().unwrap_err();
    assert!(
        matches!(a_err, ComputeError::Cycle(_)),
        "root A should surface cycle: {a_err:?}",
    );
    assert!(
        matches!(b_err, ComputeError::Cycle(_)),
        "root B should surface cycle: {b_err:?}",
    );
}

/// Cross-root cycle where only one root wraps its dependency
/// request in `with_cycle_guard`. The guarded root's key is on the
/// cycle path, so the detector fires its user guard directly and
/// that root recovers. The unguarded root's key is also on the
/// cycle path, and with no user frame open its task's synthetic
/// fallback fires, so its `ComputeEngine::compute` returns
/// `Err(Cycle(..))`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_root_catches_other_root_errors() {
    use pixi_compute_engine::DataStore;

    struct Handshake {
        g_started: Notify,
        u_started: Notify,
    }
    trait HasHandshake {
        fn handshake(&self) -> &Handshake;
    }
    impl HasHandshake for DataStore {
        fn handshake(&self) -> &Handshake {
            self.get::<Arc<Handshake>>().as_ref()
        }
    }

    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("G")]
    struct KeyG;
    impl Key for KeyG {
        type Value = Result<(), String>;
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            ctx.global_data().handshake().g_started.notify_one();
            ctx.global_data().handshake().u_started.notified().await;
            match ctx
                .with_cycle_guard(|ctx| {
                    async move {
                        let _ = ctx.compute(&KeyU).await;
                    }
                    .boxed()
                })
                .await
            {
                Ok(()) => Ok(()),
                Err(cycle) => Err(format!("caught:{cycle}")),
            }
        }
    }

    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("U")]
    struct KeyU;
    impl Key for KeyU {
        type Value = ();
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            ctx.global_data().handshake().g_started.notified().await;
            ctx.global_data().handshake().u_started.notify_one();
            ctx.compute(&KeyG).await.ok();
        }
    }

    let handshake = Arc::new(Handshake {
        g_started: Notify::new(),
        u_started: Notify::new(),
    });
    let engine = ComputeEngine::builder().with_data(handshake).build();
    let e2 = engine.clone();
    let g_task = tokio::spawn(async move { engine.compute(&KeyG).await });
    let u_task = tokio::spawn(async move { e2.compute(&KeyU).await });
    let (g_res, u_res) = tokio::join!(g_task, u_task);

    // The guarded root caught the cycle and produced a Value
    // whose inner Result is Err("caught:...").
    let g_value = g_res.unwrap().expect("guarded root should return Ok");
    let msg = g_value.expect_err("guarded root should have caught the cycle");
    assert!(msg.starts_with("caught:"), "caught message: {msg}");

    // The unguarded root surfaces the cycle to its caller.
    let u_err = u_res.unwrap().unwrap_err();
    assert!(
        matches!(u_err, ComputeError::Cycle(_)),
        "unguarded root should surface cycle: {u_err:?}",
    );
}

/// Strict semantic: a `with_cycle_guard` on a caller whose key is
/// NOT on the detected cycle path must NOT catch the cycle. The
/// guard exists only to handle cycles the caller's own key
/// participates in; a cycle that merely happens in a dependency
/// flows past the user guard and ends the caller's task via its
/// synthetic fallback, surfacing as `Err(Cycle(..))` at the engine
/// boundary.
#[tokio::test(flavor = "current_thread")]
async fn guard_on_non_cycle_key_does_not_catch_downstream_cycle() {
    // `Outer` sits above the cycle and wraps its dependency
    // request in `with_cycle_guard`. The cycle is entirely below
    // it: `Inner('A')` calls `Inner('B')` calls `Inner('A')`, with
    // no guard on either `Inner` key.
    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("{_0}")]
    struct Inner(char);
    impl Key for Inner {
        type Value = ();
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            let next = match self.0 {
                'A' => 'B',
                'B' => 'A',
                _ => return,
            };
            ctx.compute(&Inner(next)).await;
        }
    }

    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("Outer")]
    struct Outer;
    impl Key for Outer {
        /// Outer returns a sentinel that records which branch of
        /// its compute body ran. If the guard catches, we return
        /// `"caught"`; if the scope somehow finishes normally, we
        /// return `"ok"`. With strict semantics neither branch
        /// should run: the cycle must end Outer's task via its
        /// synthetic fallback, so this Value is never produced.
        type Value = &'static str;
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            match ctx
                .with_cycle_guard(|ctx| {
                    async move {
                        ctx.compute(&Inner('A')).await;
                    }
                    .boxed()
                })
                .await
            {
                Ok(()) => "ok",
                Err(_) => "caught",
            }
        }
    }

    let engine = ComputeEngine::new();
    let err = engine.compute(&Outer).await.unwrap_err();
    assert!(
        matches!(err, ComputeError::Cycle(_)),
        "Outer's guard should not have caught a cycle below it; \
         expected Err(Cycle), got: {err:?}",
    );
}

/// A `with_cycle_guard` scope wrapping a `compute2` call catches
/// a cycle that closes inside one of the branches, because a
/// branch's `ctx.compute(&dep)` captures the innermost guard
/// visible from its stack (its own local user frames, then the
/// parent's frames, then the task fallback) at edge-creation time.
/// With no branch-local user guard open, the captured target is
/// the outer guard, and that is the scope that is notified.
#[tokio::test(flavor = "current_thread")]
async fn outer_guard_catches_cycle_from_within_branch() {
    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("M")]
    struct M;
    impl Key for M {
        type Value = Result<(), String>;
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            let caught = ctx
                .with_cycle_guard(|ctx| {
                    async move {
                        ctx.compute2(
                            // Cycling branch: self-loops on M.
                            |ctx| {
                                async move {
                                    let _ = ctx.compute(&M).await;
                                }
                                .boxed()
                            },
                            // Non-cycling sibling.
                            |_ctx| async move {}.boxed(),
                        )
                        .await;
                    }
                    .boxed()
                })
                .await;
            match caught {
                Ok(()) => Ok(()),
                Err(cycle) => Err(format!("outer:{cycle}")),
            }
        }
    }

    let engine = ComputeEngine::new();
    let v = engine.compute(&M).await.unwrap();
    let msg = v.expect_err("outer guard should catch the cycle");
    assert!(msg.starts_with("outer:"), "got: {msg}");
}

/// A `with_cycle_guard` installed inside branch A does NOT catch a
/// cycle created only by branch B. Branch B's `ctx.compute` captures
/// its own scope (which has no user guard), so the cycle is routed
/// to the task's fallback and surfaces at the engine boundary as
/// `Err(Cycle)`. Branch A's guard is never notified.
#[tokio::test(flavor = "current_thread")]
async fn branch_guard_does_not_catch_sibling_branch_cycle() {
    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("Leaf")]
    struct Leaf;
    impl Key for Leaf {
        type Value = u32;
        async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {
            42
        }
    }

    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("Loop")]
    struct SelfLoop;
    impl Key for SelfLoop {
        type Value = ();
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            ctx.compute(&SelfLoop).await;
        }
    }

    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("R")]
    struct R;
    impl Key for R {
        type Value = ();
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            ctx.compute2(
                // Branch A: non-cycling work inside its own guard.
                |ctx| {
                    async move {
                        let _ = ctx
                            .with_cycle_guard(|ctx| {
                                async move {
                                    let _ = ctx.compute(&Leaf).await;
                                }
                                .boxed()
                            })
                            .await;
                    }
                    .boxed()
                },
                // Branch B: closes a cycle, no user guard.
                |ctx| async move { ctx.compute(&SelfLoop).await }.boxed(),
            )
            .await;
        }
    }

    let engine = ComputeEngine::new();
    let err = engine.compute(&R).await.unwrap_err();
    assert!(
        matches!(err, ComputeError::Cycle(_)),
        "branch B's cycle should surface as Err(Cycle) since branch A's guard doesn't own it: {err:?}",
    );
}

/// Two sibling parallel branches both await `ctx.compute(&Shared)`
/// with their own `with_cycle_guard` open; `Shared` cycles back to
/// the parent via `ctx.compute(&Root)`. Under the single-threaded
/// runtime both branches install their edges (synchronously, while
/// `futures::future::join` polls them in turn) before `Shared`'s
/// task gets to run, so both edges are present at detection time
/// and both branches' guards fire.
///
/// This pins two invariants:
///
/// 1. The active-edge graph is a multigraph keyed per record, not
///    per `(caller, target)` pair: the second branch's record does
///    not overwrite the first's.
/// 2. Notifying targets on a cycle walks every record on each
///    traversed pair.
///
/// Under a multi-threaded runtime the spawned `Shared` task can
/// race ahead of the second branch's poll and close the cycle
/// before the second edge is installed. In that case the second
/// branch sees the cycle transitively via its `shared.await` and
/// routes to the task fallback rather than its own user guard.
/// See the module-level "Concurrency and cycle identity" note for
/// the full contract.
#[tokio::test(flavor = "current_thread")]
async fn sibling_branches_on_same_dep_both_catch_cycle() {
    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("Root")]
    struct Root;
    impl Key for Root {
        type Value = (String, String);
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            ctx.compute2(
                |ctx| {
                    async move {
                        match ctx
                            .with_cycle_guard(|ctx| {
                                async move {
                                    ctx.compute(&Shared).await;
                                }
                                .boxed()
                            })
                            .await
                        {
                            Ok(()) => "a-ok".to_string(),
                            Err(cycle) => format!("a-caught:{cycle}"),
                        }
                    }
                    .boxed()
                },
                |ctx| {
                    async move {
                        match ctx
                            .with_cycle_guard(|ctx| {
                                async move {
                                    ctx.compute(&Shared).await;
                                }
                                .boxed()
                            })
                            .await
                        {
                            Ok(()) => "b-ok".to_string(),
                            Err(cycle) => format!("b-caught:{cycle}"),
                        }
                    }
                    .boxed()
                },
            )
            .await
        }
    }

    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("Shared")]
    struct Shared;
    impl Key for Shared {
        type Value = ();
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            // Close the cycle: Shared -> Root -> Shared.
            ctx.compute(&Root).await;
        }
    }

    let engine = ComputeEngine::new();
    let (a, b) = engine.compute(&Root).await.unwrap();
    assert!(a.starts_with("a-caught:"), "branch a should catch: {a}");
    assert!(b.starts_with("b-caught:"), "branch b should catch: {b}");
}

/// One parallel branch cycles, the other does clean work. The
/// cycling branch's guard catches; the clean branch must finish
/// unaffected. Ensures a branch's guard firing doesn't leak into
/// a sibling.
#[tokio::test(flavor = "current_thread")]
async fn cycling_branch_does_not_disturb_sibling() {
    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("Leaf")]
    struct Leaf;
    impl Key for Leaf {
        type Value = u32;
        async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {
            42
        }
    }

    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("R")]
    struct R;
    impl Key for R {
        type Value = (String, u32);
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            ctx.compute2(
                |ctx| {
                    async move {
                        match ctx
                            .with_cycle_guard(|ctx| {
                                async move {
                                    let _ = ctx.compute(&R).await;
                                }
                                .boxed()
                            })
                            .await
                        {
                            Ok(()) => "no-cycle".to_string(),
                            Err(cycle) => format!("caught:{cycle}"),
                        }
                    }
                    .boxed()
                },
                |ctx| async move { ctx.compute(&Leaf).await }.boxed(),
            )
            .await
        }
    }

    let engine = ComputeEngine::new();
    let (branch_cycling, branch_clean) = engine.compute(&R).await.unwrap();
    assert!(
        branch_cycling.starts_with("caught:"),
        "cycling branch should have caught: {branch_cycling}",
    );
    assert_eq!(
        branch_clean, 42,
        "clean branch should have completed normally",
    );
}

/// When a key's compute body catches a cycle via `with_cycle_guard`
/// and folds the error into a `Value`, that folded `Value` is the
/// key's cached result. A subsequent `engine.compute(&Same)` hits
/// the cache and returns the same value without re-running compute.
///
/// Cycle recovery is part of the produced `Value`, so it is
/// indistinguishable from any other computed value from the cache's
/// point of view. Worth pinning because it means guarded cycles
/// effectively memoize on the cycle's presence.
#[tokio::test(flavor = "current_thread")]
async fn guarded_cycle_recovery_is_cached() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COMPUTE_CALLS: AtomicUsize = AtomicUsize::new(0);

    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("Recovering")]
    struct Recovering;
    impl Key for Recovering {
        type Value = Result<(), String>;
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            COMPUTE_CALLS.fetch_add(1, Ordering::SeqCst);
            ctx.with_cycle_guard(|ctx| ctx.compute(&Recovering).boxed())
                .await
                .unwrap_or_else(|cycle| Err(format!("cycle:{cycle}")))
        }
    }

    let engine = ComputeEngine::new();
    let first = engine.compute(&Recovering).await.unwrap();
    let second = engine.compute(&Recovering).await.unwrap();

    assert!(
        matches!(&first, Err(msg) if msg.starts_with("cycle:")),
        "first call should have caught and folded: {first:?}",
    );
    assert_eq!(first, second, "second call should hit the cache");
    assert_eq!(
        COMPUTE_CALLS.load(Ordering::SeqCst),
        1,
        "compute body must run exactly once across the two engine calls",
    );
}
