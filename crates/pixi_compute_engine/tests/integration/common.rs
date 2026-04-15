//! Shared helpers and Key types used across the engine integration tests.

use std::{
    hash::{Hash, Hasher},
    ops::Deref,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::Poll,
};

use derive_more::Display;
use pixi_compute_engine::{ComputeCtx, Key};

/// A wrapper that carries a side-channel value alongside a Key without
/// disturbing the Key's identity: any two `Invisible<T>` compare equal and
/// hash to the same bucket. Tests use `Invisible<Arc<AtomicUsize>>` and
/// `Invisible<Arc<AtomicBool>>` to thread counters / flags into a Key
/// while the engine continues to treat two Keys with the same logical
/// fields (but different `Arc` clones) as identical.
#[derive(Clone, Debug, Default)]
pub struct Invisible<T>(pub T);

impl<T> Deref for Invisible<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}
impl<T> PartialEq for Invisible<T> {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
impl<T> Eq for Invisible<T> {}
impl<T> Hash for Invisible<T> {
    fn hash<H: Hasher>(&self, _: &mut H) {}
}

/// An invocation counter shared between a test and its Keys.
pub type Counter = Invisible<Arc<AtomicUsize>>;

/// A boolean flag shared between a test and its Keys.
pub type Flag = Invisible<Arc<AtomicBool>>;

pub fn counter() -> Counter {
    Invisible(Arc::new(AtomicUsize::new(0)))
}
pub fn flag() -> Flag {
    Invisible(Arc::new(AtomicBool::new(false)))
}

/// A Key whose compute doubles its id. The `counter` field is side-channel
/// only and does not affect identity.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{id}")]
pub struct DoubleKey {
    pub id: u32,
    pub counter: Counter,
}
impl Key for DoubleKey {
    type Value = u32;
    async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {
        self.counter.fetch_add(1, Ordering::SeqCst);
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
