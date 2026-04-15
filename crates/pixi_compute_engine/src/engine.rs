//! The top-level [`ComputeEngine`] and its internal state.

use std::sync::Arc;

use crate::{ComputeCtx, ComputeEngineBuilder, ComputeError, Key, dedup::DedupStore};

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
#[derive(Default)]
pub(crate) struct EngineInner {
    pub(crate) store: DedupStore,
    /// Set via [`ComputeEngineBuilder::sequential_branches`]. When
    /// `true`, the parallel combinators on [`ComputeCtx`] run their
    /// branches one at a time in mint order instead of concurrently.
    pub(crate) sequential_branches: bool,
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
    /// - [`ComputeError::Cycle`] if `key` participates in a dependency
    ///   cycle (direct or transitive).
    /// - [`ComputeError::Canceled`] if the underlying spawned task was
    ///   aborted before producing a value. This happens when the final
    ///   subscriber to an in-flight compute drops its handle.
    ///
    /// The returned future uses precise capture (`use<K>`), so temporary
    /// key references like `engine.compute(&MyKey(..))` work seamlessly.
    ///
    pub fn compute<K: Key>(
        &self,
        key: &K,
    ) -> impl Future<Output = Result<K::Value, ComputeError>> + use<K> {
        let mut ctx = ComputeCtx::new(self.inner.clone());
        ctx.compute(key)
    }
}
