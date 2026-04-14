//! Cancellation behavior: spawn-driven progress, abort-when-no-subscribers,
//! and `try_*` short-circuit cancellation.
//!
//! These tests synchronize via [`tokio::sync::Notify`] and a drop-guard
//! rather than timers, so they run deterministically without relying on
//! wall-clock waits.

use std::{fmt, sync::Arc, sync::atomic::Ordering};

use futures::FutureExt;
use pixi_compute_engine::{ComputeCtx, ComputeEngine, Key};
use tokio::sync::Notify;

use super::common::{Counter, Flag, Invisible, counter, flag, poll_once};

/// A Key whose compute parks forever. It notifies `started` right after
/// entering the body, installs a drop-guard that notifies `dropped` when
/// the future is released (normal completion or abort), and only flips
/// `finished = true` if allowed to run past the park, which should never
/// happen in these tests.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct SlowKey {
    id: u32,
    started: Invisible<Arc<Notify>>,
    dropped: Invisible<Arc<Notify>>,
    finished: Flag,
}
impl fmt::Display for SlowKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.id)
    }
}
impl Key for SlowKey {
    type Value = u32;
    async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {
        struct DropNotify(Arc<Notify>);
        impl Drop for DropNotify {
            fn drop(&mut self) {
                self.0.notify_one();
            }
        }
        let _guard = DropNotify(self.dropped.0.clone());
        self.started.notify_one();
        // Park forever. The task is expected to be aborted externally,
        // at which point `_guard` drops and notifies `dropped`.
        std::future::pending::<()>().await;
        self.finished.store(true, Ordering::SeqCst);
        42
    }
}

fn notify() -> Invisible<Arc<Notify>> {
    Invisible(Arc::new(Notify::new()))
}

/// When the only subscriber is dropped mid-compute, the spawned task is
/// aborted. We synchronize via `Notify`: the test waits for `started`
/// before aborting and for `dropped` after, proving the abort actually
/// took effect rather than observing a transient state.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_drops_task() {
    let engine = ComputeEngine::new();
    let started = notify();
    let dropped = notify();
    let finished = flag();
    let key = SlowKey {
        id: 1,
        started: started.clone(),
        dropped: dropped.clone(),
        finished: finished.clone(),
    };

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
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct YieldingKey {
    id: u32,
    counter: Counter,
}
impl fmt::Display for YieldingKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.id)
    }
}
impl Key for YieldingKey {
    type Value = u32;
    async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {
        self.counter.fetch_add(1, Ordering::SeqCst);
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
    let engine = ComputeEngine::new();
    let counter = counter();

    // Handshakes: A signals "subscribed", B signals "subscribed",
    // the test tells A "okay to drop now".
    let (a_ready_tx, a_ready_rx) = tokio::sync::oneshot::channel();
    let (b_ready_tx, b_ready_rx) = tokio::sync::oneshot::channel();
    let (a_drop_tx, a_drop_rx) = tokio::sync::oneshot::channel();

    let caller_a = tokio::spawn({
        let engine = engine.clone();
        let counter = counter.clone();
        async move {
            let key = YieldingKey { id: 42, counter };
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
        let counter = counter.clone();
        async move {
            let key = YieldingKey { id: 42, counter };
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
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

/// `try_compute2` cancels the still-running branch as soon as the other
/// branch returns `Err`. The slow sub-compute's drop-guard fires once
/// the try-join drops the losing branch, proving the spawned task was
/// aborted rather than merely orphaned.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn try_compute2_cancels_losing_branch() {
    #[derive(Clone, Debug, Hash, PartialEq, Eq)]
    struct Racer {
        slow: SlowKey,
    }
    impl fmt::Display for Racer {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("racer")
        }
    }
    impl Key for Racer {
        type Value = Result<u32, &'static str>;
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            let slow_started = self.slow.started.0.clone();
            let slow = self.slow.clone();
            ctx.try_compute2(
                move |ctx| {
                    ctx.compute(&slow)
                        .map(|r| r.map_err(|_| "compute-err"))
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

    let started = notify();
    let dropped = notify();
    let finished = flag();
    let slow = SlowKey {
        id: 99,
        started: started.clone(),
        dropped: dropped.clone(),
        finished: finished.clone(),
    };

    let engine = ComputeEngine::new();
    let result = engine.compute(&Racer { slow }).await.unwrap();
    assert_eq!(result, Err("fast-fail"));

    // Wait until the losing branch's spawned compute future has dropped,
    // confirming the abort actually propagated.
    dropped.notified().await;
    assert!(
        !finished.load(Ordering::SeqCst),
        "losing branch's spawned compute should have been canceled"
    );
}
