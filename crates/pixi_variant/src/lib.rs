//! This crate defines variant values for conda-build variant configurations.
//!
//! Variants are used in conda-build to specify different build configurations.
//! They can be strings (e.g., "3.11" for python version), integers (e.g., 1 for feature flags),
//! or booleans (e.g., true/false for optional features).

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

impl<'a> From<&'a str> for VariantValue {
    fn from(value: &'a str) -> Self {
        VariantValue::String(value.to_owned())
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

#[cfg(feature = "rattler_lock")]
impl From<rattler_lock::VariantValue> for VariantValue {
    fn from(value: rattler_lock::VariantValue) -> Self {
        match value {
            rattler_lock::VariantValue::String(s) => VariantValue::String(s),
            rattler_lock::VariantValue::Int(i) => VariantValue::Int(i),
            rattler_lock::VariantValue::Bool(b) => VariantValue::Bool(b),
        }
    }
}

#[cfg(feature = "rattler_lock")]
impl From<VariantValue> for rattler_lock::VariantValue {
    fn from(value: VariantValue) -> Self {
        match value {
            VariantValue::String(s) => rattler_lock::VariantValue::String(s),
            VariantValue::Int(i) => rattler_lock::VariantValue::Int(i),
            VariantValue::Bool(b) => rattler_lock::VariantValue::Bool(b),
        }
    }
}

#[cfg(feature = "rattler_lock")]
impl PartialEq<rattler_lock::VariantValue> for VariantValue {
    fn eq(&self, other: &rattler_lock::VariantValue) -> bool {
        match (self, other) {
            (VariantValue::String(a), rattler_lock::VariantValue::String(b)) => a == b,
            (VariantValue::Int(a), rattler_lock::VariantValue::Int(b)) => a == b,
            (VariantValue::Bool(a), rattler_lock::VariantValue::Bool(b)) => a == b,
            _ => false,
        }
    }
}

#[cfg(feature = "rattler_lock")]
impl PartialEq<VariantValue> for rattler_lock::VariantValue {
    fn eq(&self, other: &VariantValue) -> bool {
        other == self
    }
}

#[cfg(feature = "pixi_build_types")]
impl From<pixi_build_types::VariantValue> for VariantValue {
    fn from(value: pixi_build_types::VariantValue) -> Self {
        match value {
            pixi_build_types::VariantValue::String(s) => VariantValue::String(s),
            pixi_build_types::VariantValue::Int(i) => VariantValue::Int(i),
            pixi_build_types::VariantValue::Bool(b) => VariantValue::Bool(b),
        }
    }
}

#[cfg(feature = "pixi_build_types")]
impl From<VariantValue> for pixi_build_types::VariantValue {
    fn from(value: VariantValue) -> Self {
        match value {
            VariantValue::String(s) => pixi_build_types::VariantValue::String(s),
            VariantValue::Int(i) => pixi_build_types::VariantValue::Int(i),
            VariantValue::Bool(b) => pixi_build_types::VariantValue::Bool(b),
        }
    }
}

#[cfg(feature = "pixi_build_types")]
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

#[cfg(feature = "pixi_build_types")]
impl PartialEq<VariantValue> for pixi_build_types::VariantValue {
    fn eq(&self, other: &VariantValue) -> bool {
        other == self
    }
}
