//! Stable hashing wrapper for map-like iterables.
//!
//! `StableMap` hashes any iterator over entries `(&K, &V)` that implements
//! `ExactSizeIterator`. It stores the iterator by value and clones it during
//! hashing to avoid consuming the original.
//!
//! Notes:
//! - Key order matters (no sorting performed).
//! - The hash includes a type discriminant, entry count, then for each entry a
//!   discriminant for the key and value followed by their hashes.
//! - Keys and values must implement `Hash`.

use std::hash::{Hash, Hasher};

use crate::{FieldDiscriminant, IsDefault};

/// Stable hasher that stores an iterator of entries by value.
pub struct StableMap<I> {
    iter: I,
}

impl<I> StableMap<I> {
    /// Create a new `StableMap` from an iterator over `(&K, &V)`.
    pub fn new(iter: I) -> Self {
        Self { iter }
    }
}

impl<K, V, I> Hash for StableMap<I>
where
    I: Clone + ExactSizeIterator<Item = (K, V)>,
    K: Hash,
    V: IsDefault,
    <V as IsDefault>::Item: Hash,
{
    fn hash<H: Hasher>(&self, state: &mut H) {
        FieldDiscriminant::new("map").hash(state);
        let iter = self.iter.clone();
        iter.len().hash(state);
        for (k, v) in iter {
            if let Some(value) = v.is_non_default() {
                FieldDiscriminant::new("map:key").hash(state);
                k.hash(state);
                FieldDiscriminant::new("map:val").hash(state);
                value.hash(state);
            }
        }
    }
}

impl<I> IsDefault for StableMap<I>
where
    I: Clone + ExactSizeIterator,
{
    type Item = Self;

    fn is_non_default(&self) -> Option<&Self::Item> {
        if self.iter.len() == 0 {
            None
        } else {
            Some(self)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, hash::Hasher};

    use ordermap::OrderMap;
    use xxhash_rust::xxh3::Xxh3;

    use super::*;
    use crate::StableHashBuilder;

    #[test]
    fn btreemap_same_contents_same_hash() {
        let mut m1: BTreeMap<&str, i32> = BTreeMap::new();
        m1.insert("a", 1);
        m1.insert("b", 2);

        let mut m2: BTreeMap<&str, i32> = BTreeMap::new();
        m2.insert("b", 2);
        m2.insert("a", 1);

        let mut h1 = Xxh3::default();
        StableMap::new(m1.iter()).hash(&mut h1);
        let r1 = h1.finish();

        let mut h2 = Xxh3::default();
        StableMap::new(m2.iter()).hash(&mut h2);
        let r2 = h2.finish();

        // BTreeMap iteration is ordered by key; both maps yield same order
        assert_eq!(r1, r2);
    }

    #[test]
    fn ordermap_order_affects_hash() {
        let mut m1: OrderMap<&str, i32> = OrderMap::new();
        m1.insert("a", 1);
        m1.insert("b", 2);

        let mut m2: OrderMap<&str, i32> = OrderMap::new();
        m2.insert("b", 2);
        m2.insert("a", 1);

        let mut h1 = Xxh3::default();
        StableMap::new(m1.iter()).hash(&mut h1);
        let r1 = h1.finish();

        let mut h2 = Xxh3::default();
        StableMap::new(m2.iter()).hash(&mut h2);
        let r2 = h2.finish();

        // OrderMap preserves insertion order; hashes should differ
        assert_ne!(r1, r2);
    }

    #[test]
    fn length_is_part_of_hash() {
        let mut m1: BTreeMap<&str, i32> = BTreeMap::new();
        m1.insert("a", 1);

        let mut m2: BTreeMap<&str, i32> = BTreeMap::new();
        m2.insert("a", 1);
        m2.insert("b", 2);

        let mut h1 = Xxh3::default();
        StableMap::new(m1.iter()).hash(&mut h1);
        let r1 = h1.finish();

        let mut h2 = Xxh3::default();
        StableMap::new(m2.iter()).hash(&mut h2);
        let r2 = h2.finish();

        assert_ne!(r1, r2);
    }

    #[test]
    fn empty_map_is_default_skipped_by_builder() {
        let m1: BTreeMap<&str, i32> = BTreeMap::new();
        let mut h_empty = Xxh3::default();
        StableHashBuilder::<'_, _>::new()
            .field("m", &StableMap::new(m1.iter()))
            .finish(&mut h_empty);
        let r_empty = h_empty.finish();

        let mut h_baseline = Xxh3::default();
        StableHashBuilder::<'_, _>::new().finish(&mut h_baseline);
        let r_baseline = h_baseline.finish();

        assert_eq!(r_empty, r_baseline);
    }
}
