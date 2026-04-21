//! The top-level [`ComputeEngine`] and its internal state.

use std::sync::Arc;

use crate::{
    ComputeCtx, ComputeEngineBuilder, ComputeError, DataStore, InjectedKey, Key,
    cycle::active_edges::ActiveEdges,
    key_graph::{KeyGraph, Lookup},
};

/// The top-level compute engine.
///
/// An engine is a handle (internally `Arc`-based) and can be freely cloned.
/// All clones share the same dedup / completed-value cache, so two handles
/// handed to different tasks will observe each other's results.
///
/// # Lifecycle
///
/// 1. Create an engine with [`ComputeEngine::new`] (or [`Default`]).
/// 2. From any async context backed by a tokio runtime, call
///    [`ComputeEngine::compute`] with a reference to a [`Key`]. The engine
///    returns the cached value if one exists, joins an in-flight compute
///    if one is running, or spawns a fresh compute otherwise.
/// 3. Clone the engine freely to share across tasks; all clones see the
///    same cache.
///
/// # Runtime requirements
///
/// The engine uses [`tokio::spawn`] to drive each compute. All calls to
/// [`ComputeEngine::compute`] must happen from within a tokio runtime.
///
/// # Example
///
/// ```
/// use std::fmt;
/// use pixi_compute_engine::{ComputeCtx, ComputeEngine, Key};
///
/// #[derive(Clone, Debug, Hash, PartialEq, Eq)]
/// struct Double(u32);
///
/// impl fmt::Display for Double {
///     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
///         write!(f, "{}", self.0)
///     }
/// }
///
/// impl Key for Double {
///     type Value = u32;
///     async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {
///         self.0 * 2
///     }
/// }
///
/// # tokio_test::block_on(async {
/// let engine = ComputeEngine::new();
/// assert_eq!(engine.compute(&Double(21)).await.unwrap(), 42);
///
/// // A clone shares the cache: the second compute does not re-run.
/// let shared = engine.clone();
/// assert_eq!(shared.compute(&Double(21)).await.unwrap(), 42);
/// # });
/// ```
#[derive(Clone)]
pub struct ComputeEngine {
    pub(crate) inner: Arc<EngineInner>,
}

/// Engine state shared between every [`ComputeEngine`] clone and every
/// [`ComputeCtx`] spawned by a compute.
pub(crate) struct EngineInner {
    /// Keyed graph storage. Doubles as the value cache and the
    /// dependency graph.
    pub(crate) graph: KeyGraph,
    /// Global active-edge graph used by synchronous cycle detection
    /// in [`ComputeCtx::compute`](crate::ComputeCtx::compute). Each
    /// edge carries the notify target that was active when the edge
    /// was created, so detection does not need a separate per-key
    /// guard registry to route cycles to cross-task scopes.
    pub(crate) active_edges: Arc<ActiveEdges>,
    /// Set via [`ComputeEngineBuilder::sequential_branches`]. When
    /// `true`, the parallel combinators on [`ComputeCtx`] run their
    /// branches one at a time in mint order instead of concurrently.
    pub(crate) sequential_branches: bool,
    /// Engine-wide shared data, set at construction time.
    pub(crate) global_data: DataStore,
}

impl Default for ComputeEngine {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl ComputeEngine {
    /// Create a fresh engine with an empty cache and default settings.
    ///
    /// Equivalent to [`Default::default`]. The engine holds no tokio
    /// resources until the first [`compute`](Self::compute) call.
    /// For non-default settings (e.g. serialized sub-compute ordering
    /// for tests), use [`ComputeEngine::builder`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Start building a [`ComputeEngine`] with non-default settings.
    pub fn builder() -> ComputeEngineBuilder {
        ComputeEngineBuilder::new()
    }

    /// Compute the value for `key`, deduping against any in-flight or
    /// previously-completed compute for the same key.
    ///
    /// # Caching behavior
    ///
    /// - **Completed cache hit**: the stored value is cloned and returned.
    /// - **In-flight**: the existing shared future is joined; compute
    ///   runs exactly once, all subscribers receive the same result.
    /// - **Miss**: a fresh tokio task is spawned to run the compute; its
    ///   shared future is installed in the in-flight cache, and on
    ///   completion the value is promoted to the completed cache.
    ///
    /// # Errors
    ///
    /// - [`ComputeError::Canceled`] if the underlying spawned task was
    ///   aborted before producing a value. This happens when the final
    ///   subscriber to an in-flight compute drops its handle.
    /// - [`ComputeError::Cycle`] if a dependency cycle was detected
    ///   that no
    ///   [`ComputeCtx::with_cycle_guard`](crate::ComputeCtx::with_cycle_guard)
    ///   scope on the cycle path caught. The wrapped
    ///   [`CycleError`](crate::CycleError) carries the full ring of
    ///   keys.
    ///
    /// The returned future uses precise capture (`use<K>`), so temporary
    /// key references like `engine.compute(&MyKey(..))` work seamlessly.
    ///
    /// # Do not call from within a compute body
    ///
    /// The root ctx this method builds has no `current` key, so any
    /// edges it would add are not seen by cycle detection. A nested
    /// call from inside a running [`Key::compute`](crate::Key::compute)
    /// body can therefore create a cross-task dedup deadlock that the
    /// detector will not catch.
    ///
    /// Inside a `Key::compute` body, use
    /// [`ComputeCtx::compute`](crate::ComputeCtx::compute), which
    /// does participate in cycle detection.
    pub fn compute<K: Key>(
        &self,
        key: &K,
    ) -> impl Future<Output = Result<K::Value, ComputeError>> + use<K> {
        let mut ctx = ComputeCtx::new(self.inner.clone());
        ctx.compute_root(key)
    }

    /// Inject a value for an [`InjectedKey`].
    ///
    /// The value is stored directly in the engine's graph. Subsequent
    /// [`compute`](Self::compute) calls (or
    /// [`ComputeCtx::compute`](crate::ComputeCtx::compute) inside a
    /// Key's compute body) for this key will return the injected value
    /// immediately without spawning a task.
    ///
    /// # Panics
    ///
    /// Panics if `key` has already been injected on this engine.
    /// Overwriting is forbidden because the engine has no invalidation
    /// mechanism: computed keys that already read the old value would
    /// silently hold stale cached results.
    ///
    /// # Example
    ///
    /// ```
    /// use std::fmt;
    /// use pixi_compute_engine::{ComputeEngine, InjectedKey};
    ///
    /// #[derive(Clone, Debug, Hash, PartialEq, Eq)]
    /// struct Seed(u32);
    ///
    /// impl fmt::Display for Seed {
    ///     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    ///         write!(f, "{}", self.0)
    ///     }
    /// }
    ///
    /// impl InjectedKey for Seed {
    ///     type Value = u64;
    /// }
    ///
    /// let engine = ComputeEngine::new();
    /// engine.inject(Seed(1), 42);
    /// ```
    pub fn inject<K: InjectedKey>(&self, key: K, value: K::Value) {
        self.inner.graph.insert_injected::<K>(&key, value);
    }

    /// Read an injected key synchronously, without recording a
    /// dependency.
    ///
    /// Returns `None` if the key has not been injected yet. Unlike
    /// [`ComputeCtx::compute`](crate::ComputeCtx::compute) (which
    /// panics on a missing injected key), this method is safe to call
    /// for optional lookups or pre-flight checks.
    ///
    /// # Caution
    ///
    /// This method does **not** record a dependency. Using it inside
    /// a Key's compute body would make the dependency invisible to
    /// introspection and (once invalidation exists) prevent the
    /// engine from knowing that the parent needs to recompute when
    /// the injected value changes. Use
    /// [`ComputeCtx::compute`](crate::ComputeCtx::compute) there
    /// instead.
    pub fn read<K: InjectedKey>(&self, key: &K) -> Option<K::Value> {
        match self.inner.graph.lookup::<K>(key) {
            Some(Lookup::Completed(value)) => Some(value),
            Some(Lookup::InFlight(_)) => {
                unreachable!("InjectedKey cannot be in-flight")
            }
            None => None,
        }
    }
}
