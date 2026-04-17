//! Type-erased wrapper around a [`Key`](crate::Key).
//!
//! [`AnyKey`] lets the engine store, hash, compare, and display Keys without
//! needing to know their concrete type. It is used for rendering cycle
//! stacks in error messages and for the dependency-graph snapshot, which
//! collects Keys regardless of their static type.

use std::{
    fmt,
    hash::{Hash, Hasher},
    sync::Arc,
};

use cmp_any::PartialEqAny;

use crate::{Demand, Key};

/// A type-erased, reference-counted handle to a [`Key`].
///
/// `AnyKey` is the engine's way of talking about Keys when the concrete
/// type is not statically known: rendering cycle stacks, populating
/// dependency-graph snapshots, and building heterogenous collections
/// of Keys all go through `AnyKey`.
///
/// Two `AnyKey`s compare equal iff they wrap values of the same concrete
/// `Key` type and those values compare equal under `K`'s `Eq` impl. Hashing
/// folds the wrapped type's [`TypeId`] into the hash so distinct Key types
/// with identical `Hash` impls land in different buckets.
///
/// Cloning is cheap (an `Arc` bump). `AnyKey` implements [`Display`],
/// [`Debug`], [`Hash`], [`PartialEq`], and [`Eq`].
///
/// [`TypeId`]: std::any::TypeId
/// [`Display`]: fmt::Display
/// [`Debug`]: fmt::Debug
///
/// # Example
///
/// ```
/// use std::fmt;
/// use pixi_compute_engine::{AnyKey, ComputeCtx, Key};
///
/// #[derive(Clone, Debug, Hash, PartialEq, Eq)]
/// struct MyKey(u32);
///
/// impl fmt::Display for MyKey {
///     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
///         write!(f, "{}", self.0)
///     }
/// }
///
/// impl Key for MyKey {
///     type Value = u32;
///     async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {
///         self.0
///     }
/// }
///
/// let any = AnyKey::new(MyKey(42));
/// assert_eq!(any.key_type_name(), "MyKey");
/// assert_eq!(format!("{any}"), "MyKey(42)");
///
/// // AnyKeys wrapping equal Keys of the same type compare equal.
/// assert_eq!(any, AnyKey::new(MyKey(42)));
/// ```
#[derive(Clone)]
pub struct AnyKey {
    inner: Arc<dyn AnyKeyDyn>,
}

impl AnyKey {
    /// Erase the type of a concrete [`Key`] by wrapping it in an `Arc`.
    ///
    /// The wrapped value is immutable; clones of the returned `AnyKey`
    /// all share a single allocation.
    pub fn new<K: Key>(key: K) -> Self {
        Self {
            inner: Arc::new(key),
        }
    }

    /// The short type name of the wrapped Key.
    ///
    /// Returns whatever the Key's [`Key::key_type_name`] produced at
    /// construction time. By default that is the concrete type's name
    /// stripped of its module path (see [`crate::short_type_name`]).
    pub fn key_type_name(&self) -> &'static str {
        self.inner.key_type_name()
    }

    /// Request a value of type `T` from the wrapped Key.
    ///
    /// Calls the Key's [`Key::provide`](crate::Key::provide)
    /// implementation and returns the value it provided, or `None` if
    /// the Key does not provide a value of type `T`.
    pub fn request_value<T: 'static>(&self) -> Option<T> {
        let mut slot: Option<T> = None;
        let mut demand = Demand::new_value::<T>(&mut slot);
        self.inner.dyn_provide(&mut demand);
        slot
    }

    /// Request a reference of type `&T` from the wrapped Key.
    ///
    /// Calls the Key's [`Key::provide`](crate::Key::provide)
    /// implementation and returns the reference it provided, or `None`
    /// if the Key does not provide a `&T`.
    pub fn request_ref<T: ?Sized + 'static>(&self) -> Option<&T> {
        let mut slot: Option<&T> = None;
        let mut demand = Demand::new_ref::<T>(&mut slot);
        self.inner.dyn_provide(&mut demand);
        slot
    }
}

impl fmt::Debug for AnyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AnyKey({}: ", self.inner.key_type_name())?;
        self.inner.dyn_debug(f)?;
        write!(f, ")")
    }
}

impl fmt::Display for AnyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Prefix with the short type name so identically-Displayed keys of
        // different types are distinguishable in error messages.
        write!(f, "{}(", self.inner.key_type_name())?;
        self.inner.dyn_display(f)?;
        write!(f, ")")
    }
}

impl Hash for AnyKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Fold the TypeId into the hash so identically-hashed values of
        // different Key types land in different buckets.
        self.inner.cmp_token().type_id().hash(state);
        self.inner.dyn_hash(state);
    }
}

impl PartialEq for AnyKey {
    fn eq(&self, other: &Self) -> bool {
        self.inner.cmp_token() == other.inner.cmp_token()
    }
}

impl Eq for AnyKey {}

/// Object-safe trait blanket-implemented for every concrete [`Key`]. All
/// operations on [`AnyKey`] go through this trait.
pub(crate) trait AnyKeyDyn: Send + Sync {
    fn key_type_name(&self) -> &'static str;
    fn cmp_token(&self) -> PartialEqAny<'_>;
    fn dyn_hash(&self, state: &mut dyn Hasher);
    fn dyn_display(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result;
    fn dyn_debug(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result;
    fn dyn_provide<'a, 's>(&'a self, demand: &mut Demand<'a, 's>);
}

impl<K: Key> AnyKeyDyn for K {
    fn key_type_name(&self) -> &'static str {
        K::key_type_name()
    }

    fn cmp_token(&self) -> PartialEqAny<'_> {
        PartialEqAny::new(self)
    }

    fn dyn_hash(&self, mut state: &mut dyn Hasher) {
        // `&mut dyn Hasher` is Sized and implements `Hasher` via the standard
        // library's forwarding impl, so the generic `K::hash` can call into it.
        self.hash(&mut state);
    }

    fn dyn_display(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }

    fn dyn_debug(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }

    fn dyn_provide<'a, 's>(&'a self, demand: &mut Demand<'a, 's>) {
        Key::provide(self, demand);
    }
}
