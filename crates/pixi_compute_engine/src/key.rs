//! The [`Key`] trait. A `Key` is the unit of work in the compute engine.

use std::{
    fmt::{Debug, Display},
    future::Future,
    hash::Hash,
};

use crate::{ComputeCtx, short_type_name};

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
/// `ctx.compute` call sites via [`ComputeError`](crate::ComputeError); the
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
}
