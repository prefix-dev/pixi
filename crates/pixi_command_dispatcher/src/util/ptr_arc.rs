//! [`PtrArc<T>`]: an `Arc<T>` newtype whose `Hash` / `Eq` / `PartialEq`
//! compare by pointer identity instead of delegating to the inner value.
//!
//! Use this for `Arc`-wrapped fields that flow through recursive
//! compute-engine Keys (e.g. `installed_source_hints`): the caller
//! builds one `Arc` at the top and every nested key clones that same
//! `Arc`, so pointer equality is the right dedup invariant. Equal-but-
//! distinct `Arc`s are treated as distinct, which produces a re-compute
//! rather than a silent cache hit across unrelated callers.
//!
//! Deref targets `Arc<T>` (not `T` directly) because that lets callers
//! `.clone()` cheaply and the usual `Arc`-through-`.deref()` pattern
//! still reaches `T`.

use std::{
    fmt,
    hash::{Hash, Hasher},
    ops::Deref,
    sync::Arc,
};

/// `Arc<T>` with pointer-identity `Hash` / `Eq`.
pub struct PtrArc<T>(Arc<T>);

impl<T> PtrArc<T> {
    pub fn new(inner: Arc<T>) -> Self {
        Self(inner)
    }

    pub fn from_value(value: T) -> Self {
        Self(Arc::new(value))
    }

    pub fn into_inner(self) -> Arc<T> {
        self.0
    }

    pub fn as_arc(&self) -> &Arc<T> {
        &self.0
    }
}

impl<T> Clone for PtrArc<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> Deref for PtrArc<T> {
    type Target = Arc<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> Hash for PtrArc<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state);
    }
}

impl<T> PartialEq for PtrArc<T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl<T> Eq for PtrArc<T> {}

impl<T: fmt::Debug> fmt::Debug for PtrArc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PtrArc").field(&*self.0).finish()
    }
}

// Intentionally no blanket `Default` impl: the generic implementation
// would allocate a fresh `Arc` per call, so two `PtrArc::default()`
// invocations would fail the pointer-identity `Eq` that is the whole
// point of this wrapper. Types that want `PtrArc<Self>: Default`
// should provide an explicit impl that returns a shared singleton
// `Arc`, e.g. via a `OnceLock`. See `InstalledSourceHints`.

impl<T> From<T> for PtrArc<T> {
    fn from(value: T) -> Self {
        Self::from_value(value)
    }
}

impl<T> From<Arc<T>> for PtrArc<T> {
    fn from(value: Arc<T>) -> Self {
        Self(value)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn same_arc_hashes_and_compares_equal() {
        let a = PtrArc::from_value(42);
        let clone = a.clone();
        assert_eq!(a, clone);
        let mut set = HashSet::new();
        set.insert(a);
        assert!(set.contains(&clone));
    }

    #[test]
    fn distinct_arcs_with_equal_content_compare_distinct() {
        let a = PtrArc::from_value(42);
        let b = PtrArc::from_value(42);
        assert_ne!(a, b);
    }

    #[test]
    fn deref_reaches_inner() {
        let a: PtrArc<i32> = 7.into();
        // Deref -> Arc<i32>, then Arc deref -> i32.
        assert_eq!(**a, 7);
    }
}
