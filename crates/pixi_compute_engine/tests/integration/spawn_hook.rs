//! Tests for [`SpawnHook`].
//!
//! Verifies that a hook is invoked for every compute-task spawn and
//! that it can install a task-local (captured from the calling task)
//! into the spawned task's context.

use std::sync::Arc;

use derive_more::Display;
use futures::future::BoxFuture;
use pixi_compute_engine::{ComputeCtx, ComputeEngine, DataStore, Key, SpawnHook};

tokio::task_local! {
    static CURRENT_TAG: Option<&'static str>;
}

#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("ReadTag")]
struct ReadTag;

impl Key for ReadTag {
    type Value = Option<&'static str>;
    async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {
        CURRENT_TAG.try_with(|c| *c).ok().flatten()
    }
}

/// Hook that snapshots `CURRENT_TAG` from the calling task and scopes
/// the spawned future with it.
struct CaptureTagHook;

impl SpawnHook for CaptureTagHook {
    fn wrap(&self, _data: &DataStore, fut: BoxFuture<'static, ()>) -> BoxFuture<'static, ()> {
        let captured = CURRENT_TAG.try_with(|c| *c).ok().flatten();
        Box::pin(CURRENT_TAG.scope(captured, fut))
    }
}

#[tokio::test(flavor = "current_thread")]
async fn hook_propagates_task_local_across_spawn() {
    let engine = ComputeEngine::builder()
        .with_spawn_hook(Arc::new(CaptureTagHook))
        .build();

    // Drive the top-level compute inside a CURRENT_TAG scope. The hook
    // must capture "hello" and re-install it in the spawned task, so
    // the Key sees it.
    let got = CURRENT_TAG
        .scope(Some("hello"), async {
            engine.compute(&ReadTag).await.unwrap()
        })
        .await;
    assert_eq!(got, Some("hello"));
}

#[tokio::test(flavor = "current_thread")]
async fn hook_captures_empty_when_caller_has_no_scope() {
    let engine = ComputeEngine::builder()
        .with_spawn_hook(Arc::new(CaptureTagHook))
        .build();

    // Caller has no CURRENT_TAG scope, so the hook's try_with returns
    // Err and the spawned task sees None.
    let got = engine.compute(&ReadTag).await.unwrap();
    assert_eq!(got, None);
}

#[tokio::test(flavor = "current_thread")]
async fn with_ctx_propagates_task_local_across_spawn() {
    let engine = ComputeEngine::builder()
        .with_spawn_hook(Arc::new(CaptureTagHook))
        .build();

    let got = CURRENT_TAG
        .scope(Some("from-with-ctx"), async {
            engine
                .with_ctx(async |ctx| ctx.compute(&ReadTag).await)
                .await
                .expect("with_ctx should succeed")
        })
        .await;

    assert_eq!(got, Some("from-with-ctx"));
}

/// Hook that counts how many times `wrap` is called.
struct CountingHook {
    count: Arc<std::sync::atomic::AtomicUsize>,
}

impl SpawnHook for CountingHook {
    fn wrap(&self, _data: &DataStore, fut: BoxFuture<'static, ()>) -> BoxFuture<'static, ()> {
        self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        fut
    }
}

#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("Trivial({})", _0)]
struct Trivial(u32);

impl Key for Trivial {
    type Value = u32;
    async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {
        self.0 * 2
    }
}

#[tokio::test(flavor = "current_thread")]
async fn hook_runs_once_per_unique_key_due_to_dedup() {
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let engine = ComputeEngine::builder()
        .with_spawn_hook(Arc::new(CountingHook {
            count: count.clone(),
        }))
        .build();

    engine.compute(&Trivial(1)).await.unwrap();
    engine.compute(&Trivial(1)).await.unwrap();
    engine.compute(&Trivial(2)).await.unwrap();

    // Two distinct keys => two spawns => two hook invocations.
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 2);
}
