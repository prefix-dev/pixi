//! Stable hashing utilities for `serde_json::Value`.
//!
//! This module provides `StableJson`, a lightweight wrapper around a
//! `serde_json::Value` that implements `Hash` with configurable behavior.
//!
//! - Uses variant discriminants to avoid collisions across JSON types
//! - Recurses through arrays/objects, including lengths and keys
//! - Optionally sorts object keys to make object hashing order-insensitive

use std::hash::{Hash, Hasher};

use serde_json::{Number, Value};

use crate::{FieldDiscriminant, IsDefault};

/// A reference-based hasher for `serde_json::Value` with configuration.
#[derive(Clone, Copy, Debug)]
pub struct StableJson<'a> {
    value: &'a Value,
    sort_keys: bool,
}

impl<'a> StableJson<'a> {
    /// Create a new stable JSON hasher with default config (sort keys).
    pub fn new(value: &'a Value) -> Self {
        Self {
            value,
            sort_keys: true,
        }
    }

    /// Configure whether to sort object keys before hashing.
    pub fn with_sort_keys(mut self, sort_keys: bool) -> Self {
        self.sort_keys = sort_keys;
        self
    }
}

impl Hash for StableJson<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        hash_value(self.value, self.sort_keys, state);
    }
}

fn hash_value<H: Hasher>(v: &Value, sort_keys: bool, state: &mut H) {
    match v {
        Value::Null => {
            FieldDiscriminant::new("json:null").hash(state);
        }
        Value::Bool(b) => {
            FieldDiscriminant::new("json:bool").hash(state);
            b.hash(state);
        }
        Value::Number(n) => {
            FieldDiscriminant::new("json:number").hash(state);
            hash_number(n, state);
        }
        Value::String(s) => {
            FieldDiscriminant::new("json:string").hash(state);
            s.hash(state);
        }
        Value::Array(arr) => {
            FieldDiscriminant::new("json:array").hash(state);
            arr.len().hash(state);
            for item in arr {
                FieldDiscriminant::new("json:array:elt").hash(state);
                hash_value(item, sort_keys, state);
            }
        }
        Value::Object(map) => {
            FieldDiscriminant::new("json:object").hash(state);
            if sort_keys {
                let mut entries: Vec<(&str, &Value)> =
                    map.iter().map(|(k, v)| (k.as_str(), v)).collect();
                entries.sort_by(|a, b| a.0.cmp(b.0));
                entries.len().hash(state);
                for (k, v) in entries {
                    FieldDiscriminant::new("json:object:key").hash(state);
                    k.hash(state);
                    FieldDiscriminant::new("json:object:val").hash(state);
                    hash_value(v, sort_keys, state);
                }
            } else {
                map.len().hash(state);
                for (k, v) in map {
                    FieldDiscriminant::new("json:object:key").hash(state);
                    k.hash(state);
                    FieldDiscriminant::new("json:object:val").hash(state);
                    hash_value(v, sort_keys, state);
                }
            }
        }
    }
}

fn hash_number<H: Hasher>(n: &Number, state: &mut H) {
    if let Some(i) = n.as_i64() {
        FieldDiscriminant::new("json:number:i64").hash(state);
        i.hash(state);
    } else if let Some(u) = n.as_u64() {
        FieldDiscriminant::new("json:number:u64").hash(state);
        u.hash(state);
    } else if let Some(f) = n.as_f64() {
        FieldDiscriminant::new("json:number:f64").hash(state);
        // Normalize -0.0 to +0.0 for a canonical zero representation
        let f = if f == 0.0 { 0.0 } else { f };
        f.to_bits().hash(state);
    } else {
        // Fallback: shouldn't happen for valid JSON numbers
        FieldDiscriminant::new("json:number:other").hash(state);
        n.to_string().hash(state);
    }
}

impl IsDefault for StableJson<'_> {
    type Item = Self;

    fn is_non_default(&self) -> Option<&Self::Item> {
        if !self.value.is_null() {
            Some(self)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::hash::Hasher;

    use xxhash_rust::xxh3::Xxh3;

    use super::*;

    #[test]
    fn object_key_order_ignored_when_sorting() {
        let v1 = serde_json::json!({ "b": 2, "a": 1, "c": [1, 2]});
        let v2 = serde_json::json!({ "c": [1, 2], "a": 1, "b": 2});

        let mut h1 = Xxh3::default();
        StableJson::new(&v1).with_sort_keys(true).hash(&mut h1);
        let r1 = h1.finish();

        let mut h2 = Xxh3::default();
        StableJson::new(&v2).with_sort_keys(true).hash(&mut h2);
        let r2 = h2.finish();

        assert_eq!(r1, r2);
    }

    #[test]
    fn object_key_order_ignored_by_default() {
        let v1 = serde_json::json!({ "y": 2, "x": 1 });
        let v2 = serde_json::json!({ "x": 1, "y": 2 });

        let mut h1 = Xxh3::default();
        StableJson::new(&v1).hash(&mut h1);
        let r1 = h1.finish();

        let mut h2 = Xxh3::default();
        StableJson::new(&v2).hash(&mut h2);
        let r2 = h2.finish();

        assert_eq!(r1, r2);
    }

    #[test]
    fn array_order_affects_hash() {
        let v1 = serde_json::json!([1, 2, 3]);
        let v2 = serde_json::json!([3, 2, 1]);

        let mut h1 = Xxh3::default();
        StableJson::new(&v1).hash(&mut h1);
        let r1 = h1.finish();

        let mut h2 = Xxh3::default();
        StableJson::new(&v2).hash(&mut h2);
        let r2 = h2.finish();

        assert_ne!(r1, r2);
    }

    #[test]
    fn different_types_do_not_collide() {
        let s = serde_json::json!("1");
        let n = serde_json::json!(1);

        let mut h1 = Xxh3::default();
        StableJson::new(&s).hash(&mut h1);
        let r1 = h1.finish();

        let mut h2 = Xxh3::default();
        StableJson::new(&n).hash(&mut h2);
        let r2 = h2.finish();

        assert_ne!(r1, r2);
    }
}
