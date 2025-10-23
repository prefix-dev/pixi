use std::fmt::Display;
use std::{cmp::Ordering, collections::BTreeMap};

use serde::{Deserialize, Serialize};

/// A collection of key-value pairs representing selected conda-build variants for a package.
#[derive(Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SelectedVariant(pub BTreeMap<String, VariantValue>);

impl<K: Into<String>, V: Into<VariantValue>, I: IntoIterator<Item = (K, V)>> From<I>
    for SelectedVariant
{
    fn from(iter: I) -> Self {
        Self(
            iter.into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        )
    }
}

impl<V: From<VariantValue>> From<SelectedVariant> for BTreeMap<String, V> {
    fn from(selected: SelectedVariant) -> Self {
        selected
            .0
            .into_iter()
            .map(|(k, v)| (k, V::from(v)))
            .collect()
    }
}

impl SelectedVariant {
    pub fn iter(&self) -> impl Iterator<Item = (&String, &VariantValue)> {
        self.0.iter()
    }
}

impl PartialEq<BTreeMap<String, pixi_build_types::VariantValue>> for SelectedVariant {
    fn eq(&self, other: &BTreeMap<String, pixi_build_types::VariantValue>) -> bool {
        if self.0.len() != other.len() {
            return false;
        }
        for (key, value) in &self.0 {
            match other.get(key) {
                Some(other_value) if value == other_value => continue,
                _ => return false,
            }
        }
        true
    }
}

impl std::fmt::Debug for SelectedVariant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.0.iter()).finish()
    }
}

///
/// Variants are used in conda-build to specify different build configurations.
/// They can be strings (e.g., "3.11" for python version), integers (e.g., 1 for feature flags),
/// or booleans (e.g., true/false for optional features).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum VariantValue {
    /// String variant value (most common, e.g., python version "3.11")
    String(String),
    /// Integer variant value (e.g., for numeric feature flags)
    Int(i64),
    /// Boolean variant value (e.g., for on/off features)
    Bool(bool),
}

impl PartialOrd for VariantValue {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for VariantValue {
    fn cmp(&self, other: &Self) -> Ordering {
        #[allow(clippy::match_same_arms)]
        match (self, other) {
            (VariantValue::String(a), VariantValue::String(b)) => a.cmp(b),
            (VariantValue::Int(a), VariantValue::Int(b)) => a.cmp(b),
            (VariantValue::Bool(a), VariantValue::Bool(b)) => a.cmp(b),
            // Define ordering between different types for deterministic sorting
            (VariantValue::String(_), _) => Ordering::Less,
            (_, VariantValue::String(_)) => Ordering::Greater,
            (VariantValue::Int(_), VariantValue::Bool(_)) => Ordering::Less,
            (VariantValue::Bool(_), VariantValue::Int(_)) => Ordering::Greater,
        }
    }
}

impl Display for VariantValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VariantValue::String(s) => write!(f, "{}", s),
            VariantValue::Int(i) => write!(f, "{}", i),
            VariantValue::Bool(b) => write!(f, "{}", b),
        }
    }
}

impl From<pixi_build_types::VariantValue> for VariantValue {
    fn from(value: pixi_build_types::VariantValue) -> Self {
        match value {
            pixi_build_types::VariantValue::String(s) => VariantValue::String(s),
            pixi_build_types::VariantValue::Int(i) => VariantValue::Int(i),
            pixi_build_types::VariantValue::Bool(b) => VariantValue::Bool(b),
        }
    }
}

impl From<VariantValue> for pixi_build_types::VariantValue {
    fn from(value: VariantValue) -> Self {
        match value {
            VariantValue::String(s) => Self::String(s),
            VariantValue::Int(i) => Self::Int(i),
            VariantValue::Bool(b) => Self::Bool(b),
        }
    }
}

impl From<VariantValue> for pixi_record::VariantValue {
    fn from(value: VariantValue) -> Self {
        match value {
            VariantValue::String(s) => pixi_record::VariantValue::String(s),
            VariantValue::Int(i) => pixi_record::VariantValue::Int(i),
            VariantValue::Bool(b) => pixi_record::VariantValue::Bool(b),
        }
    }
}

impl From<pixi_record::VariantValue> for VariantValue {
    fn from(value: pixi_record::VariantValue) -> Self {
        match value {
            pixi_record::VariantValue::String(s) => Self::String(s),
            pixi_record::VariantValue::Int(i) => Self::Int(i),
            pixi_record::VariantValue::Bool(b) => Self::Bool(b),
        }
    }
}

impl PartialEq<pixi_build_types::VariantValue> for VariantValue {
    fn eq(&self, other: &pixi_build_types::VariantValue) -> bool {
        match (self, other) {
            (VariantValue::String(a), pixi_build_types::VariantValue::String(b)) => a == b,
            (VariantValue::Int(a), pixi_build_types::VariantValue::Int(b)) => a == b,
            (VariantValue::Bool(a), pixi_build_types::VariantValue::Bool(b)) => a == b,
            _ => false,
        }
    }
}

impl PartialEq<pixi_record::VariantValue> for VariantValue {
    fn eq(&self, other: &pixi_record::VariantValue) -> bool {
        match (self, other) {
            (VariantValue::String(a), pixi_record::VariantValue::String(b)) => a == b,
            (VariantValue::Int(a), pixi_record::VariantValue::Int(b)) => a == b,
            (VariantValue::Bool(a), pixi_record::VariantValue::Bool(b)) => a == b,
            _ => false,
        }
    }
}
