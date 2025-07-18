//! Stable hash builder for creating consistent hash implementations.
//!
//! This module provides tools for creating hash implementations that:
//! - Only include non-default field values to maintain forward/backward compatibility
//! - Process fields in alphabetical order for consistency
//! - Prevent hash collisions between different field configurations
//! - Use direct references without intermediate hashing for efficiency

use ordermap::OrderMap;
use rattler_digest::digest::generic_array::GenericArray;
use std::collections::BTreeMap;
use std::hash::Hash;

/// A field discriminant used in hash implementations to ensure different field
/// configurations produce different hashes while maintaining forward/backward compatibility.
///
/// This type wraps a static string that identifies which field is being hashed,
/// preventing hash collisions when the same value appears in different fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FieldDiscriminant(&'static str);

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

/// Trait to determine if a value should be considered "default" and thus skipped in hash calculations.
/// This helps maintain forward/backward compatibility by only including discriminants for meaningful values.
pub(crate) trait IsDefault {
    fn is_default(&self) -> bool;
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

/// Builder pattern for creating stable hash implementations that automatically handle
/// field discriminants, default value detection, and alphabetical ordering.
pub(crate) struct StableHashBuilder<'a, H: std::hash::Hasher> {
    fields: BTreeMap<&'static str, &'a dyn DynHashable<H>>,
}

impl<'a, H: std::hash::Hasher> StableHashBuilder<'a, H> {
    /// Create a new StableHashBuilder.
    pub fn new() -> Self {
        Self {
            fields: Default::default(),
        }
    }

    /// Add a field to the hash if it's not in its default state.
    /// Fields will be automatically sorted alphabetically before hashing.
    pub fn field<T: Hash + IsDefault>(mut self, name: &'static str, value: &'a T) -> Self {
        if !value.is_default() {
            self.fields.insert(name, value);
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

impl<K, V> IsDefault for OrderMap<K, V> {
    fn is_default(&self) -> bool {
        self.is_empty()
    }
}

impl IsDefault for String {
    fn is_default(&self) -> bool {
        false // Never skip required string fields
    }
}

impl IsDefault for url::Url {
    fn is_default(&self) -> bool {
        false // Never skip required URL fields
    }
}

impl IsDefault for std::path::PathBuf {
    fn is_default(&self) -> bool {
        false // Never skip PathBuf fields
    }
}

impl<T> IsDefault for Vec<T> {
    fn is_default(&self) -> bool {
        self.is_empty()
    }
}

impl IsDefault for rattler_conda_types::Version {
    fn is_default(&self) -> bool {
        false // Never skip version fields
    }
}

impl IsDefault for rattler_conda_types::StringMatcher {
    fn is_default(&self) -> bool {
        false // Never skip StringMatcher fields
    }
}

impl IsDefault for rattler_conda_types::BuildNumberSpec {
    fn is_default(&self) -> bool {
        false // Never skip BuildNumberSpec fields
    }
}

impl IsDefault for rattler_conda_types::VersionSpec {
    fn is_default(&self) -> bool {
        false // Never skip VersionSpec fields
    }
}

impl<U, T: rattler_digest::digest::generic_array::ArrayLength<U>> IsDefault for GenericArray<U, T> {
    fn is_default(&self) -> bool {
        false // Never skip digest output fields
    }
}
