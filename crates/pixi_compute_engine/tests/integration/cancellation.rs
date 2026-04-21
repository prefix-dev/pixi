//! Cancellation behavior: spawn-driven progress, abort-when-no-subscribers,
//! and `try_*` short-circuit cancellation.
//!
//! These tests synchronize via [`tokio::sync::Notify`] and a drop-guard
//! rather than timers, so they run deterministically without relying on
//! wall-clock waits.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use derive_more::Display;
use futures::FutureExt;
use pixi_compute_engine::{ComputeCtx, ComputeEngine, DataStore, Key};
use tokio::sync::Notify;

use super::common::{HasTestCounter, poll_once, test_counter, test_flag};

/// Test data for [`SlowKey`]: synchronization handles stored in the
/// engine's DataStore.
struct SlowKeyData {
    started: Arc<Notify>,
    dropped: Arc<Notify>,
    finished: Arc<AtomicBool>,
}

trait HasSlowKeyData {
    fn slow_key_data(&self) -> &SlowKeyData;
}

impl HasSlowKeyData for DataStore {
    fn slow_key_data(&self) -> &SlowKeyData {
        self.get::<SlowKeyData>()
    }
}

/// A Key whose compute parks forever. It notifies `started` right after
/// entering the body, installs a drop-guard that notifies `dropped` when
/// the future is released (normal completion or abort), and only flips
/// `finished = true` if allowed to run past the park, which should never
/// happen in these tests.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{id}")]
struct SlowKey {
    id: u32,
}
impl Key for SlowKey {
    type Value = u32;
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let data = ctx.global_data().slow_key_data();
        struct DropNotify(Arc<Notify>);
        impl Drop for DropNotify {
            fn drop(&mut self) {
                self.0.notify_one();
            }
        }
        let _guard = DropNotify(data.dropped.clone());
        data.started.notify_one();
        // Park forever. The task is expected to be aborted externally,
        // at which point `_guard` drops and notifies `dropped`.
        std::future::pending::<()>().await;
        data.finished.store(true, Ordering::SeqCst);
        42
    }
}

/// When the only subscriber is dropped mid-compute, the spawned task is
/// aborted. We synchronize via `Notify`: the test waits for `started`
/// before aborting and for `dropped` after, proving the abort actually
/// took effect rather than observing a transient state.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_drops_task() {
    let started = Arc::new(Notify::new());
    let dropped = Arc::new(Notify::new());
    let finished = Arc::new(AtomicBool::new(false));
    let engine = ComputeEngine::builder()
        .with_data(SlowKeyData {
            started: started.clone(),
            dropped: dropped.clone(),
            finished: finished.clone(),
        })
        .build();
    let key = SlowKey { id: 1 };

    let caller = tokio::spawn({
        let e = engine.clone();
        async move { e.compute(&key).await }
    });

    // Wait until the spawned compute has actually begun.
    started.notified().await;

    caller.abort();
    let _ = caller.await;

    // Wait until the compute future has been dropped, which is what
    // happens once the abort propagates through the Shared's refcount.
    dropped.notified().await;
    assert!(
        !finished.load(Ordering::SeqCst),
        "task ran to completion instead of being aborted"
    );
}

/// A Key whose compute yields a few times, giving the test time to
/// interleave a `select!`-racing caller and a normal caller.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{id}")]
struct YieldingKey {
    id: u32,
}
impl Key for YieldingKey {
    type Value = u32;
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        ctx.global_data()
            .test_counter()
            .fetch_add(1, Ordering::SeqCst);
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        self.id * 3
    }
}

/// When one subscriber wraps the compute in a cancellable arm and drops
/// its branch after another subscriber has joined, the remaining
/// subscriber still receives the value: progress is spawn-driven, not
/// poll-driven.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_with_subscribers() {
    let counter = test_counter();
    let engine = ComputeEngine::builder().with_data(counter.clone()).build();

    // Handshakes: A signals "subscribed", B signals "subscribed",
    // the test tells A "okay to drop now".
    let (a_ready_tx, a_ready_rx) = tokio::sync::oneshot::channel();
    let (b_ready_tx, b_ready_rx) = tokio::sync::oneshot::channel();
    let (a_drop_tx, a_drop_rx) = tokio::sync::oneshot::channel();

    let caller_a = tokio::spawn({
        let engine = engine.clone();
        async move {
            let key = YieldingKey { id: 42 };
            let mut compute = engine.compute(&key).boxed();
            // Poll once to spawn the task and register as a subscriber.
            poll_once(&mut compute).await;
            let _ = a_ready_tx.send(());
            // Wait for the signal to drop, then drop the compute future
            // (which decrements the subscriber count).
            let _ = a_drop_rx.await;
            drop(compute);
        }
    });

    let caller_b = tokio::spawn({
        let engine = engine.clone();
        async move {
            let key = YieldingKey { id: 42 };
            let mut compute = engine.compute(&key).boxed();
            // Poll once to subscribe to the same shared future as A.
            poll_once(&mut compute).await;
            let _ = b_ready_tx.send(());
            // Drive to completion.
            compute.await.unwrap()
        }
    });

    // Wait for both subscriptions, then signal A to drop.
    a_ready_rx.await.ok();
    b_ready_rx.await.ok();
    let _ = a_drop_tx.send(());

    let _ = caller_a.await;
    assert_eq!(caller_b.await.unwrap(), 126);
    assert_eq!(counter.0.load(Ordering::SeqCst), 1);
}

/// `try_compute2` cancels the still-running branch as soon as the other
/// branch returns `Err`. The slow sub-compute's drop-guard fires once
/// the try-join drops the losing branch, proving the spawned task was
/// aborted rather than merely orphaned.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn try_compute2_cancels_losing_branch() {
    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("racer")]
    struct Racer;
    impl Key for Racer {
        type Value = Result<u32, &'static str>;
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            let slow_started = ctx.global_data().slow_key_data().started.clone();
            ctx.try_compute2(
                move |ctx| {
                    ctx.compute(&SlowKey { id: 99 })
                        .map(Ok::<u32, &'static str>)
                        .boxed()
                },
                move |_ctx| {
                    // Wait until the slow branch has actually spawned its
                    // sub-compute, then fail. This guarantees the abort
                    // has something to cancel.
                    async move {
                        slow_started.notified().await;
                        Err::<u32, &'static str>("fast-fail")
                    }
                    .boxed()
                },
            )
            .await
            .map(|(_, b)| b)
        }
    }

    let started = Arc::new(Notify::new());
    let dropped = Arc::new(Notify::new());
    let finished = test_flag();
    let engine = ComputeEngine::builder()
        .with_data(SlowKeyData {
            started: started.clone(),
            dropped: dropped.clone(),
            finished: finished.0.clone(),
        })
        .build();

    let result = engine.compute(&Racer).await.unwrap();
    assert_eq!(result, Err("fast-fail"));

    // Wait until the losing branch's spawned compute future has dropped,
    // confirming the abort actually propagated.
    dropped.notified().await;
    assert!(
        !finished.0.load(Ordering::SeqCst),
        "losing branch's spawned compute should have been canceled"
    );
}
