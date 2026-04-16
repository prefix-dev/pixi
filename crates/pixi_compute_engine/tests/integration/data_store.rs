//! Tests for `DataStore` and `ComputeCtx::global_data`.

use std::sync::Arc;

use derive_more::Display;
use pixi_compute_engine::{ComputeCtx, ComputeEngine, DataStore, Key};

// -- DataStore unit tests --

#[test]
fn round_trip() {
    let mut store = DataStore::new();
    store.set(42u32);
    assert_eq!(*store.get::<u32>(), 42);
}

#[test]
fn try_get_returns_none_for_missing() {
    let store = DataStore::new();
    assert!(store.try_get::<String>().is_none());
}

#[test]
#[should_panic(expected = "no value was set")]
fn get_panics_on_missing() {
    let store = DataStore::new();
    let _ = store.get::<String>();
}

#[test]
#[should_panic(expected = "called twice")]
fn duplicate_set_panics() {
    let mut store = DataStore::new();
    store.set(1u32);
    store.set(2u32);
}

#[test]
fn multiple_types() {
    let mut store = DataStore::new();
    store.set(42u32);
    store.set(String::from("hello"));
    store.set(true);

    assert_eq!(*store.get::<u32>(), 42);
    assert_eq!(store.get::<String>(), "hello");
    assert!(*store.get::<bool>());
}

#[test]
fn chaining() {
    let mut store = DataStore::new();
    store.set(1u32).set(String::from("x"));
    assert_eq!(*store.get::<u32>(), 1);
    assert_eq!(store.get::<String>(), "x");
}

// -- Integration: Key reads from global_data --

#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("Greeting")]
struct GreetingKey;

/// Shared config stored in the DataStore.
struct Prefix(String);

impl Key for GreetingKey {
    type Value = String;
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let prefix = ctx.global_data().get::<Prefix>();
        format!("{}, world!", prefix.0)
    }
}

#[tokio::test(flavor = "current_thread")]
async fn key_reads_global_data() {
    let engine = ComputeEngine::builder()
        .with_data(Prefix("Hello".into()))
        .build();

    let result = engine.compute(&GreetingKey).await.unwrap();
    assert_eq!(result, "Hello, world!");
}

// -- Extension-trait pattern --

trait HasPrefix {
    fn prefix(&self) -> &Prefix;
}

impl HasPrefix for DataStore {
    fn prefix(&self) -> &Prefix {
        self.get::<Prefix>()
    }
}

#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("ExtGreeting")]
struct ExtGreetingKey;

impl Key for ExtGreetingKey {
    type Value = String;
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let prefix = ctx.global_data().prefix();
        format!("{}, extension!", prefix.0)
    }
}

#[tokio::test(flavor = "current_thread")]
async fn extension_trait_access() {
    let engine = ComputeEngine::builder()
        .with_data(Prefix("Hi".into()))
        .build();

    let result = engine.compute(&ExtGreetingKey).await.unwrap();
    assert_eq!(result, "Hi, extension!");
}

// -- Global data does not affect dedup --

/// A counter stored in global data. The Key reads it but its identity
/// does not include the counter value, so the engine should dedup.
struct SharedCounter(Arc<std::sync::atomic::AtomicUsize>);

#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("CountKey")]
struct CountKey;

impl Key for CountKey {
    type Value = u32;
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let counter = ctx.global_data().get::<SharedCounter>();
        counter.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        42
    }
}

#[tokio::test(flavor = "current_thread")]
async fn global_data_does_not_affect_dedup() {
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let engine = ComputeEngine::builder()
        .with_data(SharedCounter(counter.clone()))
        .build();

    // Compute twice with the same key.
    let a = engine.compute(&CountKey).await.unwrap();
    let b = engine.compute(&CountKey).await.unwrap();

    assert_eq!(a, 42);
    assert_eq!(b, 42);
    // Compute ran exactly once (second call was deduped).
    assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
}
