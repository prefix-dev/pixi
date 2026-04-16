//! The engine's in-memory key store. Doubles as the dependency graph:
//! each entry carries either an in-flight compute future or a completed
//! value plus its recorded dependencies, so introspection iterates the
//! same data structure that drives caching.
//!
//! The store is sharded by Key type. For each concrete `K: Key` it
//! holds a [`PerTypeSlot<K>`] containing a map of `K → GraphNode<K>`
//! with two states:
//!
//! - `InFlight`: a weak reference to the `Shared` future of a
//!   currently-running compute. Subscribers hold the corresponding
//!   strong `Shared` clones; when the last subscriber drops, the weak
//!   reference becomes dangling and the spawned task is aborted by the
//!   `AbortOnDrop` guard wrapping its `JoinHandle`.
//! - `Completed`: a value whose compute ran to completion together
//!   with the deps observed during that compute. The spawned task
//!   transitions the entry into `Completed` synchronously as its last
//!   step before returning, so any value that lands here was produced
//!   by a task that was never aborted.
//!
//! Type erasure across the outer map goes through the [`TypedSlot`]
//! trait: typed access (insert / lookup) downcasts via [`TypedSlot::as_any`],
//! type-erased iteration (introspection) goes through [`TypedSlot::snapshot`].

use std::{
    any::{Any, TypeId},
    collections::HashMap,
    sync::Arc,
};

use futures::{
    FutureExt,
    future::{BoxFuture, Shared, WeakShared},
};
use parking_lot::Mutex;

use crate::{AnyKey, ComputeError, Key, StorageType};

/// A boxed compute future for Key `K`.
pub(crate) type KeyFuture<K> = BoxFuture<'static, Result<<K as Key>::Value, ComputeError>>;

/// The shared future produced by a single compute.
pub(crate) type ComputeFuture<V> = Shared<BoxFuture<'static, Result<V, ComputeError>>>;

/// Result of looking up a Key in the store.
pub(crate) enum Lookup<V> {
    /// A previously-completed value is available immediately.
    Completed(V),
    /// A shared future is in flight; await it to receive the value.
    InFlight(ComputeFuture<V>),
}

/// Identity of a single spawn within a [`PerTypeSlot`].
///
/// A monotonically increasing per-slot counter, handed out at
/// `InFlight` install time and threaded into the spawned task. The
/// completion path uses it to verify that the slot still holds *this*
/// task's `InFlight` entry before promoting to `Completed` (rather
/// than blindly overwriting whatever is there, which would clobber a
/// fresh re-spawn that landed in the slot after the original task was
/// cancelled but before its post-`.await` completion path ran).
pub(crate) type SpawnGeneration = u64;

/// One entry in a [`PerTypeSlot`].
pub(crate) enum GraphNode<K: Key> {
    /// A compute task is running. The weak reference upgrades while at
    /// least one subscriber holds a strong `Shared` clone; once the
    /// last subscriber drops, the upgrade fails and the entry is stale.
    InFlight {
        future: WeakShared<KeyFuture<K>>,
        generation: SpawnGeneration,
    },
    /// The compute finished. `deps` is the ordered list of keys
    /// requested via `ctx.compute(..)` during the parent's compute body
    /// (in call order; duplicates from repeated reads are preserved).
    Completed { value: K::Value, deps: Vec<AnyKey> },
    /// A value injected via [`ComputeEngine::inject`](crate::ComputeEngine::inject).
    /// No compute task was ever spawned; the value was written directly.
    Injected { value: K::Value },
}

/// Cached state for a single concrete Key type.
pub(crate) struct PerTypeSlot<K: Key> {
    pub(crate) nodes: HashMap<K, GraphNode<K>>,
    /// Source for [`SpawnGeneration`] tokens. Increments on every
    /// `InFlight` install.
    next_generation: SpawnGeneration,
}

impl<K: Key> PerTypeSlot<K> {
    fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            next_generation: 0,
        }
    }

    /// Try to satisfy a lookup from this slot. Stale `InFlight`
    /// entries (last subscriber dropped, task aborted) are removed as
    /// a side effect.
    fn lookup(&mut self, key: &K) -> Option<Lookup<K::Value>> {
        match self.nodes.get(key) {
            Some(GraphNode::Completed { value, .. } | GraphNode::Injected { value }) => {
                Some(Lookup::Completed(value.clone()))
            }
            Some(GraphNode::InFlight { future, .. }) => {
                if let Some(shared) = future.upgrade() {
                    Some(Lookup::InFlight(shared))
                } else {
                    self.nodes.remove(key);
                    None
                }
            }
            None => None,
        }
    }
}

/// Erased view of a per-type slot. Carries both a typed-downcast
/// escape hatch ([`as_any`](TypedSlot::as_any)) for the insert/lookup
/// hot path and a type-erased snapshot method for introspection.
pub(crate) trait TypedSlot: Send + Sync + 'static {
    fn as_any(&self) -> &dyn Any;
    fn snapshot(&self, out: &mut Vec<NodeRecord>);
}

/// A single per-node record produced by [`TypedSlot::snapshot`]. The
/// introspection layer turns these into its own, public-facing
/// representation.
pub(crate) struct NodeRecord {
    pub key: AnyKey,
    pub state: RawNodeState,
    pub deps: Vec<AnyKey>,
}

pub(crate) enum RawNodeState {
    Computing,
    Completed,
    Injected,
}

impl<K: Key> TypedSlot for Mutex<PerTypeSlot<K>> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn snapshot(&self, out: &mut Vec<NodeRecord>) {
        let g = self.lock();
        for (key, node) in &g.nodes {
            let any_key = AnyKey::new(key.clone());
            match node {
                GraphNode::InFlight { future, .. } => {
                    if future.upgrade().is_some() {
                        out.push(NodeRecord {
                            key: any_key,
                            state: RawNodeState::Computing,
                            deps: Vec::new(),
                        });
                    }
                }
                GraphNode::Completed { deps, .. } => {
                    out.push(NodeRecord {
                        key: any_key,
                        state: RawNodeState::Completed,
                        deps: deps.clone(),
                    });
                }
                GraphNode::Injected { .. } => {
                    out.push(NodeRecord {
                        key: any_key,
                        state: RawNodeState::Injected,
                        deps: Vec::new(),
                    });
                }
            }
        }
    }
}

/// The engine's keyed graph storage.
///
/// Access is through a single outer [`Mutex`] guarding the type-id
/// map. The outer lock is held only long enough to look up or insert
/// the per-type slot's `Arc`; per-type ops then take the inner per-type
/// `Mutex` independently. No `.await` happens under either lock.
pub(crate) struct KeyGraph {
    inner: Mutex<HashMap<TypeId, Arc<dyn TypedSlot>>>,
}

impl KeyGraph {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Look up `key` in the graph. If the value is already cached
    /// (completed, injected, or in-flight), returns it immediately.
    ///
    /// On a miss, behavior depends on [`Key::storage_type`]:
    ///
    /// **Computed** keys: mints a fresh [`SpawnGeneration`], calls
    /// `make_future` (which must be quick because the per-type lock is
    /// held; a slow closure blocks every other caller for this key
    /// type), installs the resulting future as `InFlight`, and returns
    /// the strong `Shared`.
    ///
    /// **Injected** keys: panics. All injected values must be provided
    /// via [`insert_injected`](Self::insert_injected) before any
    /// compute that depends on them. Without invalidation the engine
    /// cannot retroactively update dependents that already cached a
    /// result.
    pub(crate) fn get_or_insert_with<K, F>(&self, key: &K, make_future: F) -> Lookup<K::Value>
    where
        K: Key,
        F: FnOnce(SpawnGeneration) -> KeyFuture<K>,
    {
        let slot = self.get_or_create_slot::<K>();
        let typed: &Mutex<PerTypeSlot<K>> = slot
            .as_any()
            .downcast_ref()
            .expect("type id matches by construction");
        let mut s = typed.lock();
        if let Some(hit) = s.lookup(key) {
            return hit;
        }

        // Miss. For injected keys this means the value was never
        // provided; for computed keys we spawn a fresh task.
        match K::storage_type() {
            StorageType::Injected => {
                panic!(
                    "injected key not set: {}. \
                     All injected values must be provided via \
                     ComputeEngine::inject() before computing keys \
                     that depend on them.",
                    AnyKey::new(key.clone()),
                );
            }
            StorageType::Computed => {
                let generation = s.next_generation;
                s.next_generation += 1;
                let shared = make_future(generation).shared();
                let weak = shared
                    .downgrade()
                    .expect("freshly-created Shared must be downgradeable");
                s.nodes.insert(
                    key.clone(),
                    GraphNode::InFlight {
                        future: weak,
                        generation,
                    },
                );
                Lookup::InFlight(shared)
            }
        }
    }

    /// Promote a freshly-computed value into the slot if it still
    /// holds the `InFlight` entry that was installed for this spawn
    /// (matched by [`SpawnGeneration`]). If the slot has moved on
    /// (a fresh re-spawn replaced our entry after our subscribers
    /// dropped, or a different task already promoted the value), the
    /// computed value is silently dropped: the live entry is
    /// authoritative.
    ///
    /// Called by the spawned task as its last synchronous step before
    /// returning its value. Because the insert is sync and happens
    /// after the last `.await`, cancellation cannot interrupt it.
    pub(crate) fn insert_completed<K: Key>(
        &self,
        key: &K,
        value: K::Value,
        deps: Vec<AnyKey>,
        generation: SpawnGeneration,
    ) {
        let slot = self.get_or_create_slot::<K>();
        let typed: &Mutex<PerTypeSlot<K>> = slot
            .as_any()
            .downcast_ref()
            .expect("type id matches by construction");
        let mut s = typed.lock();
        let owns_slot = matches!(
            s.nodes.get(key),
            Some(GraphNode::InFlight { generation: g, .. }) if *g == generation,
        );
        if owns_slot {
            s.nodes
                .insert(key.clone(), GraphNode::Completed { value, deps });
        }
    }

    /// Look up `key` without creating an entry on miss. Used by the
    /// injected-key path in [`ComputeCtx::compute`](crate::ComputeCtx::compute)
    /// where spawning a compute is never appropriate.
    pub(crate) fn lookup<K: Key>(&self, key: &K) -> Option<Lookup<K::Value>> {
        let map = self.inner.lock();
        let slot = map.get(&TypeId::of::<K>())?.clone();
        drop(map);
        let typed: &Mutex<PerTypeSlot<K>> = slot
            .as_any()
            .downcast_ref()
            .expect("type id matches by construction");
        let mut s = typed.lock();
        s.lookup(key)
    }

    /// Store an injected value directly, without spawning a compute.
    ///
    /// # Panics
    ///
    /// Panics if `key` already has an entry in the graph (whether
    /// injected, completed, or in-flight). The single-write contract
    /// of [`InjectedKey`](crate::InjectedKey) means each key may be
    /// injected at most once.
    pub(crate) fn insert_injected<K: Key>(&self, key: &K, value: K::Value) {
        let slot = self.get_or_create_slot::<K>();
        let typed: &Mutex<PerTypeSlot<K>> = slot
            .as_any()
            .downcast_ref()
            .expect("type id matches by construction");
        let mut s = typed.lock();
        if s.nodes.contains_key(key) {
            panic!("injected key already set: {}", AnyKey::new(key.clone()),);
        }
        s.nodes.insert(key.clone(), GraphNode::Injected { value });
    }

    /// Walk every per-type slot, invoking `sink` with each one.
    /// Used by introspection to build a snapshot.
    pub(crate) fn for_each_slot(&self, mut sink: impl FnMut(&dyn TypedSlot)) {
        // Clone the Arcs out from under the outer lock so per-type
        // locks are taken without holding the outer one.
        let arcs: Vec<Arc<dyn TypedSlot>> = self.inner.lock().values().cloned().collect();
        for slot in &arcs {
            sink(&**slot);
        }
    }

    fn get_or_create_slot<K: Key>(&self) -> Arc<dyn TypedSlot> {
        let mut map = self.inner.lock();
        map.entry(TypeId::of::<K>())
            .or_insert_with(|| Arc::new(Mutex::new(PerTypeSlot::<K>::new())) as Arc<dyn TypedSlot>)
            .clone()
    }
}

impl Default for KeyGraph {
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
