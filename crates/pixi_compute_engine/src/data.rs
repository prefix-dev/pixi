//! Type-keyed container for engine-wide shared data.

use std::{
    any::{Any, TypeId, type_name},
    collections::HashMap,
};

/// Type-keyed container for engine-wide shared data.
///
/// Each type can be stored at most once. Values are set before engine
/// construction and are immutable for the engine's lifetime.
///
/// Downstream crates define ergonomic accessors via extension traits:
///
/// ```ignore
/// pub trait HasGateway {
///     fn gateway(&self) -> &Arc<Gateway>;
/// }
///
/// impl HasGateway for DataStore {
///     fn gateway(&self) -> &Arc<Gateway> {
///         self.get::<Arc<Gateway>>()
///     }
/// }
///
/// // Inside a Key's compute body:
/// let gw = ctx.global_data().gateway();
/// ```
pub struct DataStore {
    data: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl Default for DataStore {
    fn default() -> Self {
        Self::new()
    }
}

impl DataStore {
    /// Create an empty data store.
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
        }
    }

    /// Insert a value. Returns `&mut Self` for chaining.
    ///
    /// # Panics
    ///
    /// Panics if a value of type `T` was already inserted.
    pub fn set<T: Send + Sync + 'static>(&mut self, value: T) -> &mut Self {
        let id = TypeId::of::<T>();
        if self.data.contains_key(&id) {
            panic!(
                "DataStore::set called twice for type `{}`",
                type_name::<T>()
            );
        }
        self.data.insert(id, Box::new(value));
        self
    }

    /// Get a reference to a stored value.
    ///
    /// # Panics
    ///
    /// Panics with a descriptive message if the type was not set.
    pub fn get<T: Send + Sync + 'static>(&self) -> &T {
        self.try_get::<T>().unwrap_or_else(|| {
            panic!(
                "DataStore::get::<{}>() called but no value was set for this type",
                type_name::<T>()
            )
        })
    }

    /// Try to get a reference to a stored value.
    ///
    /// Returns `None` if the type was not set.
    pub fn try_get<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.data
            .get(&TypeId::of::<T>())
            .map(|boxed| boxed.downcast_ref::<T>().expect("TypeId mismatch"))
    }
}
