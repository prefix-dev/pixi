//! The [`Key`] trait. A `Key` is the unit of work in the compute engine.

use std::{
    fmt::{Debug, Display},
    future::Future,
    hash::Hash,
};

use crate::{ComputeCtx, Demand, short_type_name};

/// How the engine stores and retrieves a Key's value.
///
/// Returned by [`Key::storage_type`]. The engine uses this to decide
/// whether to spawn a compute task or look up a pre-populated value.
///
/// Modeled after DICE's `StorageType` enum in buck2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageType {
    /// The value is produced by [`Key::compute`]. The engine spawns a
    /// task, deduplicates concurrent requests, and caches the result.
    Computed,
    /// The value is injected externally via
    /// [`ComputeEngine::inject`](crate::ComputeEngine::inject). The
    /// engine performs a lookup-only, never spawning a task. Panics if
    /// the value has not been injected yet.
    Injected,
}

/// A unit of computation.
///
/// A `Key` identifies *what* is being computed; [`Key::compute`] defines
/// *how* to compute it. The engine dedupes concurrent requests for the same
/// `Key`, caches completed values, and detects cycles among Keys that
/// depend on each other through [`ComputeCtx::compute`](crate::ComputeCtx::compute).
///
/// # Required super-traits
///
/// - [`Hash`] + [`Eq`]: identity for the dedup cache.
/// - [`Clone`]: the engine clones the Key into its internal cache entry
///   and into each sub-ctx that traverses a cycle.
/// - [`Display`] + [`Debug`]: log-friendly rendering when a Key appears in
///   error messages ([`ComputeError::Cycle`]) or graph introspection.
/// - [`Send`] + [`Sync`] + `'static`: the compute runs on a spawned
///   tokio task.
///
/// # Value must be cheap to clone
///
/// Every subscriber to a deduped compute receives its own clone of the
/// value. `Value` should therefore be something like `Arc<T>` or a newtype
/// over `Arc<T>`. Returning a large owned type like `Vec<u8>` is legal but
/// will deep-clone on every dedup hit. This is a convention, enforced by
/// code review and rustdoc rather than by a trait bound.
///
/// # Errors live inside `Value`
///
/// [`compute`](Key::compute) returns `Self::Value` directly, not
/// `Result<Self::Value, _>`. User-level failures must be modeled inside
/// `Value` (for example `Arc<Result<T, E>>` or a newtype over one).
/// Framework-level failures (cycles, cancellation) are surfaced at
/// [`ComputeCtx::compute`](crate::ComputeCtx::compute) call sites via
/// [`ComputeError`](crate::ComputeError); the
/// caller is responsible for folding them into its own `Value` if needed.
///
/// # Example
///
/// A trivial Key that squares its input:
///
/// ```
/// use std::fmt;
/// use pixi_compute_engine::{ComputeCtx, ComputeEngine, Key};
///
/// #[derive(Clone, Debug, Hash, PartialEq, Eq)]
/// struct Square(u32);
///
/// impl fmt::Display for Square {
///     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
///         write!(f, "{}", self.0)
///     }
/// }
///
/// impl Key for Square {
///     type Value = u32;
///     async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {
///         self.0 * self.0
///     }
/// }
///
/// # tokio_test::block_on(async {
/// let engine = ComputeEngine::new();
/// assert_eq!(engine.compute(&Square(7)).await.unwrap(), 49);
/// // The second call hits the completed-value cache; compute does not re-run.
/// assert_eq!(engine.compute(&Square(7)).await.unwrap(), 49);
/// # });
/// ```
///
/// A Key that depends on another Key via `ctx.compute`:
///
/// ```ignore
/// impl Key for Parent {
///     type Value = u32;
///     async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
///         let child = ctx.compute(&Square(self.0)).await.unwrap();
///         child + 1
///     }
/// }
/// ```
///
/// [`ComputeError::Cycle`]: crate::ComputeError::Cycle
pub trait Key: Hash + Eq + Clone + Display + Debug + Send + Sync + 'static {
    /// The result type of this computation.
    ///
    /// Must be cheap to clone (see the trait-level note).
    type Value: Clone + Send + Sync + 'static;

    /// Perform the computation, using `ctx` to request dependencies.
    ///
    /// Implementations may call `ctx.compute(..)` any number of times to
    /// depend on other `Key`s; the engine serializes those calls so the
    /// recorded dependency order is deterministic within a single compute
    /// frame. For explicit parallelism use `ctx.compute2` or
    /// `ctx.compute_join`.
    fn compute(&self, ctx: &mut ComputeCtx) -> impl Future<Output = Self::Value> + Send;

    /// Equality check between two values of this Key. Used for future early
    /// cutoff: when a dependency recomputes to the same value, dependents
    /// do not need to recompute.
    ///
    /// Defaults to `false` (always invalidate). Override for Keys whose
    /// values have a well-defined equality and where identical recomputes
    /// are expected often enough to matter.
    fn equality(_a: &Self::Value, _b: &Self::Value) -> bool {
        false
    }

    /// Short, log-friendly type name. Strips the module path by default.
    ///
    /// Override to return a fixed string if the auto-derived name is
    /// unstable across toolchain versions.
    fn key_type_name() -> &'static str {
        short_type_name::<Self>()
    }

    /// How the engine stores and retrieves this Key's value.
    ///
    /// Defaults to [`StorageType::Computed`]. The blanket
    /// `impl<K: InjectedKey> Key for K` overrides this to
    /// [`StorageType::Injected`].
    ///
    /// Do not override this in manual `Key` implementations:
    /// returning `Injected` without a matching
    /// [`ComputeEngine::inject`](crate::ComputeEngine::inject) call
    /// would panic on the first lookup, and the `compute` body would
    /// never run.
    #[doc(hidden)]
    fn storage_type() -> StorageType {
        StorageType::Computed
    }

    /// Expose auxiliary values through the type-erased
    /// [`AnyKey`](crate::AnyKey) surface.
    ///
    /// Default is a no-op. Override to let consumers extract
    /// domain-specific metadata from an erased Key without knowing its
    /// concrete type (e.g. a cycle-error handler walking the cycle path
    /// can call
    /// [`AnyKey::request_value`](crate::AnyKey::request_value) on each
    /// frame to pull domain context).
    ///
    /// The second [`Demand`] lifetime parameter is elided because it
    /// belongs to the caller's slot-management scheme, not to the
    /// Key. See [`Demand`] for the full API.
    fn provide<'a>(&'a self, _demand: &mut Demand<'a, '_>) {}
}
