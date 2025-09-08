use indexmap::IndexMap;
use std::hash::Hash;

/// A trait for types that support a union-style merge policy.
pub trait MergeUnion {
    fn union(&self, other: &Self) -> Self;
}

/// Merge two optional single-assignment values.
/// - If both are `Some`, return a conflict error via the provided closure.
/// - Otherwise prefer `a` when set, else `b`.
pub fn merge_single_option<T: Clone, E>(
    a: &Option<T>,
    b: &Option<T>,
    conflict: impl FnOnce(&T, &T) -> E,
) -> Result<Option<T>, E> {
    match (a, b) {
        (Some(a), Some(b)) => Err(conflict(a, b)),
        (Some(a), None) => Ok(Some(a.clone())),
        (None, Some(b)) => Ok(Some(b.clone())),
        (None, None) => Ok(None),
    }
}

/// Merge two optional lists by concatenating and de-duplicating, preserving order.
pub fn merge_list_dedup<T: Clone + Eq + Hash>(
    a: &Option<Vec<T>>,
    b: &Option<Vec<T>>,
) -> Option<Vec<T>> {
    match (a, b) {
        (Some(a), Some(b)) => {
            let set = a
                .iter()
                .cloned()
                .chain(b.iter().cloned())
                .collect::<indexmap::IndexSet<_>>();
            Some(set.into_iter().collect())
        }
        (Some(a), None) => Some(a.clone()),
        (None, Some(b)) => Some(b.clone()),
        (None, None) => None,
    }
}

/// Merge two optional maps where left (`a`) overrides right (`b`) on key conflicts.
pub fn merge_map_override_left<K, V>(
    a: &Option<IndexMap<K, V>>,
    b: &Option<IndexMap<K, V>>,
) -> Option<IndexMap<K, V>>
where
    K: Clone + Eq + Hash,
    V: Clone,
{
    match (a, b) {
        (Some(a), Some(b)) => {
            let mut merged = b.clone();
            merged.extend(a.iter().map(|(k, v)| (k.clone(), v.clone())));
            Some(merged)
        }
        (Some(a), None) => Some(a.clone()),
        (None, Some(b)) => Some(b.clone()),
        (None, None) => None,
    }
}

/// Implement `MergeUnion` for `Option<T>` where `T` itself supports `MergeUnion`.
impl<T: MergeUnion + Clone> MergeUnion for Option<T> {
    fn union(&self, other: &Self) -> Self {
        // This contains the different matches for different cases
        match (self, other) {
            (Some(a), Some(b)) => Some(a.union(b)),
            (Some(a), None) => Some(a.clone()),
            (None, Some(b)) => Some(b.clone()),
            (None, None) => None,
        }
    }
}
