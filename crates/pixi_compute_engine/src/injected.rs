//! The [`InjectedKey`] trait and its blanket [`Key`] implementation.
//!
//! An injected key is a value fed into the engine externally via
//! [`ComputeEngine::inject`](crate::ComputeEngine::inject), rather than
//! being produced by a [`Key::compute`] body. Other
//! Keys depend on injected values through the normal
//! [`ComputeCtx::compute`](crate::ComputeCtx::compute) call; the engine
//! short-circuits the lookup and returns the pre-populated value without
//! spawning a task.

use std::{
    fmt::{Debug, Display},
    hash::Hash,
};

use crate::{ComputeCtx, Key, StorageType, short_type_name};

/// A value injected into the engine by the user, not computed.
///
/// An `InjectedKey` identifies a piece of data that the engine itself does
/// not know how to produce. Instead, the user calls
/// [`ComputeEngine::inject`](crate::ComputeEngine::inject) to feed the
/// value in. Other Keys depend on it via the normal
/// [`ComputeCtx::compute`](crate::ComputeCtx::compute) call, and the
/// dependency is recorded for introspection.
///
/// # Single-write contract
///
/// Each injected key may be set at most once per engine. Calling
/// [`ComputeEngine::inject`](crate::ComputeEngine::inject) twice for the
/// same key panics. Overwriting is forbidden because the engine has no
/// invalidation mechanism: computed keys that already consumed the old
/// value would silently hold stale cached results.
///
/// # Relationship to `Key`
///
/// A blanket `impl<K: InjectedKey> Key for K` is provided, so injected
/// keys are a special case of [`Key`] and work transparently with
/// [`ComputeCtx::compute`](crate::ComputeCtx::compute). The engine
/// short-circuits the lookup (no task is spawned) and returns the
/// pre-populated value. If the value has not been injected yet, the
/// engine panics to prevent silent staleness (the engine has no
/// invalidation mechanism to retroactively update cached dependents).
///
/// This blanket impl also enforces disjointness: implementing both
/// `InjectedKey` and `Key` on the same type is a compile error (the
/// blanket impl conflicts with the manual impl).
///
/// # Value must be cheap to clone
///
/// Same convention as [`Key::Value`]: prefer `Arc<T>` or a newtype over
/// `Arc<T>`.
///
/// # Example
///
/// ```
/// use std::fmt;
/// use pixi_compute_engine::{ComputeEngine, InjectedKey};
///
/// #[derive(Clone, Debug, Hash, PartialEq, Eq)]
/// struct Config(String);
///
/// impl fmt::Display for Config {
///     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
///         write!(f, "{}", self.0)
///     }
/// }
///
/// impl InjectedKey for Config {
///     type Value = String;
/// }
///
/// let engine = ComputeEngine::new();
/// assert!(engine.read(&Config("db_url".into())).is_none());
///
/// engine.inject(Config("db_url".into()), "postgres://localhost".into());
/// assert_eq!(engine.read(&Config("db_url".into())).unwrap(), "postgres://localhost");
/// ```
pub trait InjectedKey: Hash + Eq + Clone + Display + Debug + Send + Sync + 'static {
    /// The result type of this injected value.
    ///
    /// Must be cheap to clone (see the trait-level note).
    type Value: Clone + Send + Sync + 'static;

    /// Short, log-friendly type name. Strips the module path by default.
    fn key_type_name() -> &'static str {
        short_type_name::<Self>()
    }
}

/// Blanket [`Key`] implementation for every [`InjectedKey`].
///
/// This lets injected keys participate in
/// [`ComputeCtx::compute`](crate::ComputeCtx::compute) alongside
/// regular computed keys. The engine never actually calls `compute` on
/// an injected key; it short-circuits via
/// [`StorageType::Injected`] and returns
/// the value pre-populated by
/// [`ComputeEngine::inject`](crate::ComputeEngine::inject), or panics
/// if the value was not injected.
///
/// The blanket impl also enforces that a type cannot implement both
/// `Key` and `InjectedKey`: attempting to do so produces a coherence
/// error ("conflicting implementations of trait `Key`").
impl<K: InjectedKey> Key for K {
    type Value = <K as InjectedKey>::Value;

    async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {
        panic!(
            "InjectedKey `{}` cannot be computed; \
             use ComputeEngine::inject() to provide its value",
            Self::key_type_name(),
        )
    }

    fn key_type_name() -> &'static str {
        <K as InjectedKey>::key_type_name()
    }

    fn storage_type() -> StorageType {
        StorageType::Injected
    }
}
