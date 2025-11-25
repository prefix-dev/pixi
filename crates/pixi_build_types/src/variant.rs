use std::{cmp::Ordering, fmt::Display};

use serde::{Deserialize, Deserializer, Serialize, de::Visitor};

/// Represents a conda-build variant value.
///
/// Variants are used in conda-build to specify different build configurations.
/// They can be strings (e.g., "3.11" for python version), integers (e.g., 1 for feature flags),
/// or booleans (e.g., true/false for optional features).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(untagged)]
pub enum VariantValue {
    /// String variant value (most common, e.g., python version "3.11")
    String(String),
    /// Integer variant value (e.g., for numeric feature flags)
    Int(i64),
    /// Boolean variant value (e.g., for on/off features)
    Bool(bool),
}

impl<'de> Deserialize<'de> for VariantValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct VariantValueVisitor;

        impl<'de> Visitor<'de> for VariantValueVisitor {
            type Value = VariantValue;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a string, integer, or boolean value")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(VariantValue::String(value.to_owned()))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(VariantValue::Int(value))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(VariantValue::Int(value as i64))
            }

            fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(VariantValue::Bool(value))
            }
        }

        deserializer.deserialize_any(VariantValueVisitor)
    }
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
            VariantValue::String(s) => write!(f, "{s}"),
            VariantValue::Int(i) => write!(f, "{i}"),
            VariantValue::Bool(b) => write!(f, "{b}"),
        }
    }
}

impl From<String> for VariantValue {
    fn from(value: String) -> Self {
        VariantValue::String(value)
    }
}

impl From<bool> for VariantValue {
    fn from(value: bool) -> Self {
        VariantValue::Bool(value)
    }
}

impl From<u64> for VariantValue {
    fn from(value: u64) -> Self {
        VariantValue::Int(value as i64)
    }
}
