//! Stable hash builder for creating consistent hash implementations.
//!
//! This crate provides tools for creating hash implementations that:
//! - Only include non-default field values to maintain forward/backward
//!   compatibility
//! - Process fields in alphabetical order for consistency
//! - Prevent hash collisions between different field configurations
//! - Use direct references without intermediate hashing for efficiency

use std::{collections::BTreeMap, hash::Hash};

use ordermap::OrderMap;

/// A field discriminant used in hash implementations to ensure different field
/// configurations produce different hashes while maintaining forward/backward
/// compatibility.
///
/// This type wraps a static string that identifies which field is being hashed,
/// preventing hash collisions when the same value appears in different fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FieldDiscriminant(&'static str);

impl FieldDiscriminant {
    /// Create a new field discriminant with the given field name.
    pub const fn new(field_name: &'static str) -> Self {
        Self(field_name)
    }
}

impl Hash for FieldDiscriminant {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

/// Trait to determine if a value should be considered "default" and thus
/// skipped in hash calculations. This helps maintain forward/backward
/// compatibility by only including discriminants for meaningful values.
pub trait IsDefault {
    type Item;

    fn is_non_default(&self) -> Option<&Self::Item>;
}

/// A dyn-compatible hashing trait that works with any hasher type
trait DynHashable<H: std::hash::Hasher> {
    fn dyn_hash(&self, hasher: &mut H);
}

/// Implement DynHashable for all Hash types
impl<T: Hash, H: std::hash::Hasher> DynHashable<H> for T {
    fn dyn_hash(&self, hasher: &mut H) {
        self.hash(hasher);
    }
}

/// Builder pattern for creating stable hash implementations that automatically
/// handle field discriminants, default value detection, and alphabetical
/// ordering.
pub struct StableHashBuilder<'a, H: std::hash::Hasher> {
    fields: BTreeMap<&'static str, &'a dyn DynHashable<H>>,
}

impl<H: std::hash::Hasher + Default> Default for StableHashBuilder<'_, H> {
    fn default() -> Self {
        Self {
            fields: Default::default(),
        }
    }
}

impl<'a, H: std::hash::Hasher> StableHashBuilder<'a, H> {
    /// Create a new StableHashBuilder.
    pub fn new() -> Self {
        Self {
            fields: BTreeMap::new(),
        }
    }

    /// Add a field to the hash if it's not in its default state.
    /// Fields will be automatically sorted alphabetically before hashing.
    pub fn field<T: IsDefault>(mut self, name: &'static str, value: &'a T) -> Self
    where
        T::Item: Hash,
    {
        if let Some(item) = value.is_non_default() {
            self.fields.insert(name, item);
        }
        self
    }

    /// Finish building the hash by applying all fields in alphabetical order.
    pub fn finish(self, hasher: &mut H) {
        for (key, value) in self.fields {
            FieldDiscriminant::new(key).hash(hasher);
            value.dyn_hash(hasher);
        }
    }
}

#[cfg(feature = "serde_json")]
pub mod json;

pub mod map;

impl<K, V> IsDefault for OrderMap<K, V> {
    type Item = Self;

    fn is_non_default(&self) -> Option<&Self::Item> {
        if !self.is_empty() { Some(self) } else { None }
    }
}

impl IsDefault for String {
    type Item = Self;

    fn is_non_default(&self) -> Option<&Self::Item> {
        Some(self) // Never skip required string fields
    }
}

impl IsDefault for i32 {
    type Item = Self;

    fn is_non_default(&self) -> Option<&Self::Item> {
        Some(self) // Never skip numeric fields
    }
}

impl IsDefault for std::path::PathBuf {
    type Item = Self;

    fn is_non_default(&self) -> Option<&Self::Item> {
        Some(self) // Never skip PathBuf fields
    }
}

impl<T> IsDefault for Vec<T> {
    type Item = Self;

    fn is_non_default(&self) -> Option<&Self::Item> {
        if !self.is_empty() { Some(self) } else { None }
    }
}

impl<T: IsDefault> IsDefault for Option<T> {
    type Item = T::Item;

    fn is_non_default(&self) -> Option<&Self::Item> {
        self.as_ref()?.is_non_default()
    }
}

impl<T: IsDefault> IsDefault for &T {
    type Item = T::Item;

    fn is_non_default(&self) -> Option<&Self::Item> {
        T::is_non_default(self)
    }
}

#[cfg(feature = "url")]
impl IsDefault for url::Url {
    type Item = Self;

    fn is_non_default(&self) -> Option<&Self::Item> {
        Some(self) // Never skip required URL fields
    }
}

#[cfg(feature = "rattler_conda_types")]
impl IsDefault for rattler_conda_types::Version {
    type Item = Self;

    fn is_non_default(&self) -> Option<&Self::Item> {
        Some(self) // Never skip version fields
    }
}

#[cfg(feature = "rattler_conda_types")]
impl IsDefault for rattler_conda_types::StringMatcher {
    type Item = Self;

    fn is_non_default(&self) -> Option<&Self::Item> {
        Some(self) // Never skip StringMatcher fields
    }
}

#[cfg(feature = "rattler_conda_types")]
impl IsDefault for rattler_conda_types::BuildNumberSpec {
    type Item = Self;

    fn is_non_default(&self) -> Option<&Self::Item> {
        Some(self) // Never skip BuildNumberSpec fields
    }
}

#[cfg(feature = "rattler_conda_types")]
impl IsDefault for rattler_conda_types::VersionSpec {
    type Item = Self;

    fn is_non_default(&self) -> Option<&Self::Item> {
        Some(self) // Never skip VersionSpec fields
    }
}

#[cfg(feature = "rattler_digest")]
impl<U, T: rattler_digest::digest::generic_array::ArrayLength<U>> IsDefault
    for rattler_digest::digest::generic_array::GenericArray<U, T>
{
    type Item = Self;

    fn is_non_default(&self) -> Option<&Self::Item> {
        Some(self) // Never skip digest output fields
    }
}

#[cfg(test)]
mod tests {
    use std::hash::Hasher;

    use xxhash_rust::xxh3::Xxh3;

    use super::*;

    #[test]
    fn fields_hashed_in_alphabetical_order() {
        let a = "a_val".to_string();
        let b = "b_val".to_string();

        let mut h1 = Xxh3::default();
        StableHashBuilder::<'_, _>::new()
            .field("b", &b)
            .field("a", &a)
            .finish(&mut h1);

        let mut h2 = Xxh3::default();
        StableHashBuilder::<'_, _>::new()
            .field("a", &a)
            .field("b", &b)
            .finish(&mut h2);

        assert_eq!(h1.finish(), h2.finish());
    }

    #[test]
    fn discriminants_prevent_field_collisions() {
        // Same value placed in different fields must yield different hashes
        let v = "same".to_string();

        let mut h1 = Xxh3::default();
        StableHashBuilder::<'_, _>::new()
            .field("a", &v)
            .finish(&mut h1);

        let mut h2 = Xxh3::default();
        StableHashBuilder::<'_, _>::new()
            .field("b", &v)
            .finish(&mut h2);

        assert_ne!(h1.finish(), h2.finish());
    }

    #[test]
    fn option_and_empty_values_are_skipped() {
        // None and Some(empty vec) should be equivalent due to IsDefault impls
        let none: Option<Vec<String>> = None;
        let some_empty: Option<Vec<String>> = Some(vec![]);
        let some_val: Option<Vec<String>> = Some(vec!["x".to_string()]);

        let mut h_none = Xxh3::default();
        StableHashBuilder::<'_, _>::new()
            .field("opt", &none)
            .finish(&mut h_none);
        let hash_none = h_none.finish();

        let mut h_empty = Xxh3::default();
        StableHashBuilder::<'_, _>::new()
            .field("opt", &some_empty)
            .finish(&mut h_empty);
        let hash_empty = h_empty.finish();

        let mut h_val = Xxh3::default();
        StableHashBuilder::<'_, _>::new()
            .field("opt", &some_val)
            .finish(&mut h_val);
        let hash_val = h_val.finish();

        assert_eq!(hash_none, hash_empty);
        assert_ne!(hash_none, hash_val);
    }

    #[test]
    fn ordermap_order_affects_hash() {
        // OrderMap order is considered meaningful
        let mut m1: OrderMap<&'static str, &'static str> = OrderMap::new();
        m1.insert("k1", "v1");
        m1.insert("k2", "v2");

        let mut m2: OrderMap<&'static str, &'static str> = OrderMap::new();
        m2.insert("k2", "v2");
        m2.insert("k1", "v1");

        let mut h1 = Xxh3::default();
        StableHashBuilder::<'_, _>::new()
            .field("map", &m1)
            .finish(&mut h1);

        let mut h2 = Xxh3::default();
        StableHashBuilder::<'_, _>::new()
            .field("map", &m2)
            .finish(&mut h2);

        assert_ne!(h1.finish(), h2.finish());
    }
}
