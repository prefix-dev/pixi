//! Tests for `sequential_branches`-driven ordering of the parallel
//! combinators.
//!
//! These tests deliberately give each branch a *different* number of
//! `tokio::task::yield_now()` awaits before recording its id. With the
//! default (concurrent) engine, branches complete in reverse-yields
//! order (the least-yielding finishes first), while
//! `sequential_branches(true)` forces mint order regardless of
//! per-branch yield count. Without this asymmetry both modes produce
//! the same output (`join_all`'s natural FIFO poll order happens to
//! match the chain-gated order when all branches take the same number
//! of polls), and the test would not actually discriminate.

use std::sync::{Arc, Mutex};

use derive_more::Display;
use futures::FutureExt;
use pixi_compute_engine::{ComputeCtx, ComputeEngine, DataStore, Key};

/// Shared execution log stored in the engine's DataStore.
struct ExecutionLog(Arc<Mutex<Vec<u32>>>);

trait HasExecutionLog {
    fn execution_log(&self) -> &Arc<Mutex<Vec<u32>>>;
}

impl HasExecutionLog for DataStore {
    fn execution_log(&self) -> &Arc<Mutex<Vec<u32>>> {
        &self.get::<ExecutionLog>().0
    }
}

/// A Key that yields `yields` times before appending its id to the
/// shared execution log. A higher `yields` means the branch completes
/// in more polls.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{id}")]
struct Tag {
    id: u32,
    yields: u32,
}
impl Key for Tag {
    type Value = u32;
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        for _ in 0..self.yields {
            tokio::task::yield_now().await;
        }
        ctx.global_data()
            .execution_log()
            .lock()
            .unwrap()
            .push(self.id);
        self.id
    }
}

/// Aggregator that feeds `(id, yields)` pairs into `compute_join`. Mint
/// order matches the input order of `tags`.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("agg")]
struct Aggregator {
    tags: Vec<(u32, u32)>,
}
impl Key for Aggregator {
    type Value = Vec<u32>;
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let tags: Vec<Tag> = self
            .tags
            .iter()
            .map(|&(id, yields)| Tag { id, yields })
            .collect();
        let results = ctx
            .compute_join(tags, |ctx, tag| ctx.compute(&tag).boxed())
            .await;
        results.into_iter().map(Result::unwrap).collect()
    }
}

fn new_log() -> Arc<Mutex<Vec<u32>>> {
    Arc::new(Mutex::new(Vec::new()))
}

/// Yield counts are decreasing, so with concurrent execution the
/// last-minted branch completes first. `sequential_branches(true)` must
/// override that and still finish in mint order.
#[tokio::test(flavor = "current_thread")]
async fn compute_join_serial_overrides_completion_order() {
    let log = new_log();
    let engine = ComputeEngine::builder()
        .sequential_branches(true)
        .with_data(ExecutionLog(log.clone()))
        .build();
    let agg = Aggregator {
        tags: vec![(1, 2), (2, 1), (3, 0)],
    };
    let out = engine.compute(&agg).await.unwrap();
    assert_eq!(out, vec![1, 2, 3], "result vec is input-ordered");
    assert_eq!(
        *log.lock().unwrap(),
        vec![1, 2, 3],
        "sequential mode forces completion in mint order despite the slow branch being first",
    );
}

/// Sanity check on the asymmetry: with concurrent execution the same
/// yield profile completes in reverse (fewest-yields branch first). This
/// demonstrates the test genuinely discriminates between the two modes.
/// If both produced `[1, 2, 3]`, the sequential test above would prove
/// nothing.
#[tokio::test(flavor = "current_thread")]
async fn compute_join_concurrent_follows_yield_profile() {
    let log = new_log();
    let engine = ComputeEngine::builder()
        .with_data(ExecutionLog(log.clone()))
        .build();
    let agg = Aggregator {
        tags: vec![(1, 2), (2, 1), (3, 0)],
    };
    let out = engine.compute(&agg).await.unwrap();
    assert_eq!(out, vec![1, 2, 3], "result vec is still input-ordered");
    assert_eq!(
        *log.lock().unwrap(),
        vec![3, 2, 1],
        "concurrent execution lets the fewest-yields branch finish first",
    );
}

/// `compute2` also honors `sequential_branches(true)`. Branch 1 yields
/// twice, branch 2 yields zero times. Concurrently branch 2 records
/// first (`[2, 1]`); sequentially the chain-gate forces branch 1 first
/// (`[1, 2]`).
#[tokio::test(flavor = "current_thread")]
async fn compute2_serial_overrides_completion_order() {
    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("pair")]
    struct Pair;
    impl Key for Pair {
        type Value = (u32, u32);
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            let log = ctx.global_data().execution_log().clone();
            let log_a = log.clone();
            let log_b = log;
            ctx.compute2(
                move |_ctx| {
                    async move {
                        tokio::task::yield_now().await;
                        tokio::task::yield_now().await;
                        log_a.lock().unwrap().push(1);
                        1u32
                    }
                    .boxed()
                },
                move |_ctx| {
                    async move {
                        log_b.lock().unwrap().push(2);
                        2u32
                    }
                    .boxed()
                },
            )
            .await
        }
    }

    // Concurrent: branch 2 (no yields) wins the race.
    let log_c = new_log();
    let concurrent = ComputeEngine::builder()
        .with_data(ExecutionLog(log_c.clone()))
        .build();
    assert_eq!(concurrent.compute(&Pair).await.unwrap(), (1, 2));
    assert_eq!(*log_c.lock().unwrap(), vec![2, 1]);

    // Sequential: branch 1 must finish before branch 2 starts.
    let log_s = new_log();
    let serial = ComputeEngine::builder()
        .sequential_branches(true)
        .with_data(ExecutionLog(log_s.clone()))
        .build();
    assert_eq!(serial.compute(&Pair).await.unwrap(), (1, 2));
    assert_eq!(*log_s.lock().unwrap(), vec![1, 2]);
}

/// `sequential_branches(true)` intentionally does **not** chain-gate
/// across separate top-level `ComputeEngine::compute` calls joined
/// together by the caller. This pins that documented non-guarantee, so
/// that a future change trying to extend the setting's scope to
/// top-level calls will trip this test and be forced to be deliberate.
///
/// Semantics: three independent top-level computes with decreasing yield
/// counts complete in reverse mint order, same as under the concurrent
/// default. Callers who need top-level ordering should use sequential
/// `.await`s.
#[tokio::test(flavor = "current_thread")]
async fn sequential_branches_does_not_gate_top_level_join() {
    let log = new_log();
    let engine = ComputeEngine::builder()
        .sequential_branches(true)
        .with_data(ExecutionLog(log.clone()))
        .build();
    let a = Tag { id: 1, yields: 2 };
    let b = Tag { id: 2, yields: 1 };
    let c = Tag { id: 3, yields: 0 };
    let (ra, rb, rc) = tokio::join!(engine.compute(&a), engine.compute(&b), engine.compute(&c));
    assert_eq!((ra.unwrap(), rb.unwrap(), rc.unwrap()), (1, 2, 3));
    assert_eq!(
        *log.lock().unwrap(),
        vec![3, 2, 1],
        "top-level join! runs concurrently regardless of sequential_branches",
    );
}

/// Under `sequential_branches(true)`, `try_compute_join` short-circuits
/// when a middle branch returns `Err`. The branches *after* the failing
/// one are mid-chain: their futures were minted and hold a `prev_done`
/// receiver whose sender lives inside the dropped middle branch. This
/// test pins that those later branches' closures never run (no side
/// effects observable for them) and that the error is surfaced.
#[tokio::test(flavor = "current_thread")]
async fn sequential_try_compute_join_short_circuits() {
    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("agg")]
    struct Agg;
    impl Key for Agg {
        type Value = Result<Vec<u32>, &'static str>;
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            let log = ctx.global_data().execution_log().clone();
            ctx.try_compute_join(vec![1u32, 2, 3], |_ctx, id| {
                let log = log.clone();
                async move {
                    log.lock().unwrap().push(id);
                    if id == 2 { Err("boom at 2") } else { Ok(id) }
                }
                .boxed()
            })
            .await
        }
    }

    let log = new_log();
    let engine = ComputeEngine::builder()
        .sequential_branches(true)
        .with_data(ExecutionLog(log.clone()))
        .build();
    assert_eq!(engine.compute(&Agg).await.unwrap(), Err("boom at 2"));
    assert_eq!(
        *log.lock().unwrap(),
        vec![1, 2],
        "branch 3's closure must not run after branch 2 fails and short-circuits the try_join",
    );
}

/// Under `sequential_branches(true)`, dropping a mid-chain branch's
/// future *without polling it* must not leave later branches deadlocked
/// on that branch's `done_tx`. The dropped branch releases its sender,
/// the next branch's `prev.await` sees `RecvError`, and the chain
/// continues.
///
/// Setup: use `compute_many` to get raw per-branch futures, drop the
/// middle one, then drive the first and third with `join_all`. Both
/// must complete.
#[tokio::test(flavor = "current_thread")]
async fn sequential_dropped_mid_chain_branch_does_not_deadlock() {
    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("agg")]
    struct Agg;
    impl Key for Agg {
        type Value = Vec<u32>;
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            let log = ctx.global_data().execution_log().clone();
            let mut futs = ctx.compute_many((1u32..=3).map(|id| {
                let log = log.clone();
                ComputeCtx::declare_closure(move |_ctx: &mut ComputeCtx| {
                    let log = log.clone();
                    async move {
                        log.lock().unwrap().push(id);
                        id
                    }
                    .boxed()
                })
            }));
            // Drop the middle future before polling. Its `done_tx`
            // drops with it, so branch 3's `prev.await` resolves with
            // `RecvError` (tolerated by the chain-gate) and branch 3
            // can proceed. Branch 2's closure is never invoked and
            // never pushes `2` to the log. Use `drop(..)` rather than
            // `let _middle = ..` so the future is released immediately
            // instead of at end-of-scope.
            drop(futs.remove(1));
            futures::future::join_all(futs).await
        }
    }

    let log = new_log();
    let engine = ComputeEngine::builder()
        .sequential_branches(true)
        .with_data(ExecutionLog(log.clone()))
        .build();
    let out = engine.compute(&Agg).await.unwrap();
    assert_eq!(out, vec![1, 3]);
    assert_eq!(
        *log.lock().unwrap(),
        vec![1, 3],
        "branch 3 must run even though its `prev` sender was dropped without firing",
    );
}
