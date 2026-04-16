//! Shared helpers and Key types used across the engine integration tests.

use std::{
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::Poll,
};

use derive_more::Display;
use pixi_compute_engine::{ComputeCtx, DataStore, Key};

/// Invocation counter stored in the engine's DataStore.
#[derive(Clone)]
pub struct TestCounter(pub Arc<AtomicUsize>);

/// Boolean flag stored in the engine's DataStore.
#[derive(Clone)]
pub struct TestFlag(pub Arc<AtomicBool>);

pub fn test_counter() -> TestCounter {
    TestCounter(Arc::new(AtomicUsize::new(0)))
}

pub fn test_flag() -> TestFlag {
    TestFlag(Arc::new(AtomicBool::new(false)))
}

/// Extension trait for ergonomic access to a [`TestCounter`] in global data.
pub trait HasTestCounter {
    fn test_counter(&self) -> &Arc<AtomicUsize>;
}

impl HasTestCounter for DataStore {
    fn test_counter(&self) -> &Arc<AtomicUsize> {
        &self.get::<TestCounter>().0
    }
}

/// A Key whose compute doubles its id. Increments a [`TestCounter`]
/// from global data on each compute invocation.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{id}")]
pub struct DoubleKey {
    pub id: u32,
}
impl Key for DoubleKey {
    type Value = u32;
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        ctx.global_data()
            .test_counter()
            .fetch_add(1, Ordering::SeqCst);
        // Small yield to encourage real concurrency during dedup tests.
        tokio::task::yield_now().await;
        self.id * 2
    }
}

/// Base layer of a two-Key dependency chain: returns `id + 100`.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{_0}")]
pub struct BaseKey(pub u32);
impl Key for BaseKey {
    type Value = u32;
    async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {
        self.0 + 100
    }
}

/// Outer layer: depends on `BaseKey`, adds ten to the result.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{_0}")]
pub struct PlusTenKey(pub u32);
impl Key for PlusTenKey {
    type Value = u32;
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let base = ctx.compute(&BaseKey(self.0)).await.unwrap();
        base + 10
    }
}

/// Poll `fut` exactly once. Used by the cancellation-with-subscribers
/// test to force a subscription into the dedup store before proceeding.
pub async fn poll_once<F: Future + Unpin>(fut: &mut F) {
    futures::future::poll_fn(|cx| {
        let _ = Pin::new(&mut *fut).poll(cx);
        Poll::Ready(())
    })
    .await;
}
