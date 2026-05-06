//! Operation ids and the registry threading their parent links across
//! compute-task spawns.

use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use dashmap::DashMap;
use futures::future::BoxFuture;
use pixi_compute_engine::{DataStore, SpawnHook};
use serde::Serialize;

/// Globally-unique id for one reporter event. Allocated by
/// [`OperationRegistry::allocate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct OperationId(pub u64);

tokio::task_local! {
    /// The currently-active [`OperationId`] for the running compute task.
    /// Set inside [`OperationId::scope_active`] and propagated across
    /// engine spawns by [`OperationIdSpawnHook`].
    static CURRENT_OPERATION_ID: Option<OperationId>;
}

impl OperationId {
    /// The currently-active id on this task, if any.
    pub fn current() -> Option<Self> {
        CURRENT_OPERATION_ID.try_get().ok().flatten()
    }

    /// Run `fut` with `self` installed as the current id. Nested
    /// scopes restore the previous id when they exit.
    pub async fn scope_active<F: Future>(self, fut: F) -> F::Output {
        CURRENT_OPERATION_ID.scope(Some(self), fut).await
    }
}

/// Records the parent of every allocated [`OperationId`]. Construct one
/// per `CommandDispatcher`/engine; share the `Arc` between the dispatcher
/// and every reporter implementation that allocates ids on it.
#[derive(Default)]
pub struct OperationRegistry {
    next_id: AtomicU64,
    parents: DashMap<OperationId, Option<OperationId>>,
}

impl OperationRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Allocate a fresh id. Records the current task's
    /// [`OperationId::current`] as the new id's parent.
    pub fn allocate(&self) -> OperationId {
        let id = OperationId(self.next_id.fetch_add(1, Ordering::Relaxed));
        self.parents.insert(id, OperationId::current());
        id
    }

    /// Direct parent of `id`, or `None` for a root operation or an
    /// unknown id.
    pub fn parent_of(&self, id: OperationId) -> Option<OperationId> {
        self.parents.get(&id).and_then(|v| *v)
    }

    /// Iterator over `id`'s ancestors, from immediate parent up to the
    /// root.
    pub fn ancestors(&self, id: OperationId) -> Ancestors<'_> {
        Ancestors {
            registry: self,
            next: self.parent_of(id),
        }
    }
}

/// Iterator returned by [`OperationRegistry::ancestors`].
pub struct Ancestors<'r> {
    registry: &'r OperationRegistry,
    next: Option<OperationId>,
}

impl Iterator for Ancestors<'_> {
    type Item = OperationId;

    fn next(&mut self) -> Option<Self::Item> {
        let cur = self.next?;
        self.next = self.registry.parent_of(cur);
        Some(cur)
    }
}

/// [`SpawnHook`] that propagates the current operation id across
/// compute-task spawns. Register once on the engine builder.
pub struct OperationIdSpawnHook;

impl SpawnHook for OperationIdSpawnHook {
    fn wrap(&self, _data: &DataStore, fut: BoxFuture<'static, ()>) -> BoxFuture<'static, ()> {
        let captured = OperationId::current();
        Box::pin(CURRENT_OPERATION_ID.scope(captured, fut))
    }
}
