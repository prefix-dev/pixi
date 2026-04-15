//! Basic compute, dedup, caching, and transitive-dependency tests.

use std::sync::atomic::Ordering;

use derive_more::Display;
use pixi_compute_engine::{ComputeCtx, ComputeEngine, Key};

use super::common::{Counter, DoubleKey, PlusTenKey, counter};

/// Single compute, single caller: value matches, compute ran exactly once.
#[tokio::test(flavor = "current_thread")]
async fn basic_compute() {
    let engine = ComputeEngine::new();
    let counter = counter();
    let key = DoubleKey {
        id: 21,
        counter: counter.clone(),
    };
    assert_eq!(engine.compute(&key).await.unwrap(), 42);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

/// N concurrent callers for the same Key: one compute, N subscribers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dedup_concurrent() {
    let engine = ComputeEngine::new();
    let counter = counter();

    let mut handles = Vec::new();
    for _ in 0..16 {
        let e = engine.clone();
        let c = counter.clone();
        handles.push(tokio::spawn(async move {
            e.compute(&DoubleKey { id: 7, counter: c }).await.unwrap()
        }));
    }
    for h in handles {
        assert_eq!(h.await.unwrap(), 14);
    }
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

/// Sequential callers hit the completed-value cache, so compute only runs
/// the first time.
#[tokio::test(flavor = "current_thread")]
async fn dedup_sequential() {
    let engine = ComputeEngine::new();
    let counter = counter();
    let key = DoubleKey {
        id: 5,
        counter: counter.clone(),
    };

    assert_eq!(engine.compute(&key).await.unwrap(), 10);
    assert_eq!(engine.compute(&key).await.unwrap(), 10);
    assert_eq!(engine.compute(&key).await.unwrap(), 10);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

/// A Key whose compute always produces an `Err` in its `Value`. Used to
/// verify that failures cache the same way successes do.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{id}")]
struct FailingKey {
    id: u32,
    counter: Counter,
}
impl Key for FailingKey {
    type Value = Result<u32, String>;
    async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {
        self.counter.fetch_add(1, Ordering::SeqCst);
        Err(format!("boom({})", self.id))
    }
}

/// An `Err(..)` value is cached like any other, so repeat callers see the
/// same error without re-running compute.
#[tokio::test(flavor = "current_thread")]
async fn error_caching() {
    let engine = ComputeEngine::new();
    let counter = counter();
    let key = FailingKey {
        id: 3,
        counter: counter.clone(),
    };

    assert_eq!(
        engine.compute(&key).await.unwrap(),
        Err("boom(3)".to_string())
    );
    assert_eq!(
        engine.compute(&key).await.unwrap(),
        Err("boom(3)".to_string())
    );
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

/// A Key can depend on another Key via `ctx.compute`, and the chain
/// resolves transparently.
#[tokio::test(flavor = "current_thread")]
async fn transitive_compute() {
    let engine = ComputeEngine::new();
    assert_eq!(engine.compute(&PlusTenKey(5)).await.unwrap(), 5 + 100 + 10);
}
