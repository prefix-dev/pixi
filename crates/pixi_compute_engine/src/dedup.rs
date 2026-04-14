//! Per-key deduplication and result caching for the compute engine.
//!
//! The store is sharded by Key type. For each concrete `K: Key` it holds a
//! [`PerTypeSlot`] containing two maps:
//!
//! - `in_flight`: weak references to the `Shared` future of a
//!   currently-running compute. Subscribers hold the corresponding strong
//!   `Shared` clones; when the last subscriber drops, the weak reference
//!   becomes dangling and the spawned task is aborted by the `AbortOnDrop`
//!   guard wrapping its `JoinHandle`.
//! - `completed`: strong cache of values whose compute ran to completion.
//!   The spawned task writes into this map synchronously as its last step,
//!   so any value that lands here was produced by a task that was never
//!   aborted.

use std::{any::Any, collections::HashMap};

use anymap3::Map as AnyMap;
use futures::{
    FutureExt,
    future::{BoxFuture, Shared, WeakShared},
};
use parking_lot::Mutex;

use crate::{ComputeError, Key};

/// The shared future produced by a single compute.
pub(crate) type ComputeFuture<V> = Shared<BoxFuture<'static, Result<V, ComputeError>>>;

/// Result of looking up a Key in the store.
pub(crate) enum Lookup<V> {
    /// A previously-completed value is available immediately.
    Completed(V),
    /// A shared future is in flight; await it to receive the value.
    InFlight(ComputeFuture<V>),
}

/// A boxed compute future for Key `K`.
type KeyFuture<K> = BoxFuture<'static, Result<<K as Key>::Value, ComputeError>>;

/// Cached state for a single concrete Key type.
struct PerTypeSlot<K: Key> {
    in_flight: HashMap<K, WeakShared<KeyFuture<K>>>,
    completed: HashMap<K, K::Value>,
}

impl<K: Key> PerTypeSlot<K> {
    fn new() -> Self {
        Self {
            in_flight: HashMap::new(),
            completed: HashMap::new(),
        }
    }

    /// Try to satisfy a lookup from this slot. Stale `WeakShared` entries
    /// (last subscriber dropped, task aborted) are removed as a side
    /// effect.
    fn lookup(&mut self, key: &K) -> Option<Lookup<K::Value>> {
        if let Some(value) = self.completed.get(key) {
            return Some(Lookup::Completed(value.clone()));
        }
        if let Some(weak) = self.in_flight.get(key) {
            if let Some(shared) = weak.upgrade() {
                return Some(Lookup::InFlight(shared));
            }
            self.in_flight.remove(key);
        }
        None
    }
}

/// The type-erased store of per-type slots.
///
/// Access is serialized behind a single [`Mutex`]. Lookups are short and
/// sync (`HashMap` operations plus a weak upgrade), and no `.await` occurs
/// while the lock is held.
pub(crate) struct DedupStore {
    inner: Mutex<AnyMap<dyn Any + Send + Sync>>,
}

impl DedupStore {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(AnyMap::new()),
        }
    }

    /// Look up `key` in the store. If nothing is cached or in-flight,
    /// calls `make_future` to build a fresh compute future, installs its
    /// `WeakShared` in the in-flight map, and returns the strong `Shared`.
    ///
    /// `make_future` runs under the store's mutex, so it must be quick
    /// (e.g. `tokio::spawn` plus a small amount of wrapping).
    pub(crate) fn get_or_insert_with<K, F>(&self, key: &K, make_future: F) -> Lookup<K::Value>
    where
        K: Key,
        F: FnOnce() -> BoxFuture<'static, Result<K::Value, ComputeError>>,
    {
        let mut store = self.inner.lock();
        let slot = store
            .entry::<PerTypeSlot<K>>()
            .or_insert_with(PerTypeSlot::<K>::new);

        if let Some(hit) = slot.lookup(key) {
            return hit;
        }

        let shared = make_future().shared();
        let weak = shared
            .downgrade()
            .expect("freshly-created Shared must be downgradeable");
        slot.in_flight.insert(key.clone(), weak);
        Lookup::InFlight(shared)
    }

    /// Promote a freshly-computed value into the completed map.
    ///
    /// Called by the spawned task as its last synchronous step before
    /// returning its value. Because the insert is sync and happens after
    /// the last `.await`, cancellation cannot interrupt it.
    pub(crate) fn insert_completed<K: Key>(&self, key: &K, value: K::Value) {
        let mut store = self.inner.lock();
        let slot = store
            .entry::<PerTypeSlot<K>>()
            .or_insert_with(PerTypeSlot::<K>::new);
        slot.completed.insert(key.clone(), value);
    }
}

impl Default for DedupStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Wrap a spawned `JoinHandle` into a compute future that converts a
/// cancellation `JoinError` into [`ComputeError::Canceled`] and
/// propagates panics.
pub(crate) fn boxed_compute_future<V: Send + 'static>(
    handle: tokio::task::JoinHandle<V>,
) -> BoxFuture<'static, Result<V, ComputeError>> {
    use crate::abort_on_drop::AbortOnDrop;

    async move {
        match AbortOnDrop(handle).await {
            Ok(value) => Ok(value),
            Err(e) if e.is_cancelled() => Err(ComputeError::Canceled),
            Err(e) => std::panic::resume_unwind(e.into_panic()),
        }
    }
    .boxed()
}
