//! A validated name for an extra group.

use std::{borrow::Borrow, fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// The maximum number of characters allowed in an [`ExtraGroupName`].
pub const MAX_EXTRA_GROUP_NAME_LEN: usize = 64;

/// A validated name for an extra group, as used in
/// `package.extra-dependencies.<name>` and surfaced to consumers through
/// MatchSpec extras (`package[name]`).
///
/// A valid name matches `^[a-z0-9._+-]{1,64}$`. The name is validated on
/// construction, so holding an `ExtraGroupName` is a guarantee that the name
/// is valid (typestate).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(
    feature = "schemars",
    derive(schemars::JsonSchema),
    schemars(transparent)
)]
pub struct ExtraGroupName(String);

/// The error returned when a string is not a valid [`ExtraGroupName`].
#[derive(Debug, Error)]
pub enum InvalidExtraGroupName {
    /// The name was empty or longer than [`MAX_EXTRA_GROUP_NAME_LEN`].
    #[error(
        "extra group name must be between 1 and {MAX_EXTRA_GROUP_NAME_LEN} characters, but `{name}` has {len}"
    )]
    InvalidLength {
        /// The offending name.
        name: String,
        /// Its length in characters.
        len: usize,
    },
    /// The name contained a character outside `[a-z0-9._+-]`.
    #[error(
        "extra group name `{name}` contains the invalid character `{character}`; only lowercase letters, digits, `_`, `.`, `+` and `-` are allowed"
    )]
    InvalidCharacter {
        /// The offending name.
        name: String,
        /// The first character that was not allowed.
        character: char,
    },
}

/// Returns `true` if `c` is allowed in an [`ExtraGroupName`].
///
/// Equivalent to the character class `[a-z0-9._+-]`.
fn is_valid_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | '+' | '-')
}

impl ExtraGroupName {
    /// Creates a new `ExtraGroupName`, validating that it matches
    /// `^[a-z0-9._+-]{1,64}$`.
    pub fn new(name: impl Into<String>) -> Result<Self, InvalidExtraGroupName> {
        let name = name.into();
        let len = name.chars().count();
        if len == 0 || len > MAX_EXTRA_GROUP_NAME_LEN {
            return Err(InvalidExtraGroupName::InvalidLength { name, len });
        }
        if let Some(character) = name.chars().find(|c| !is_valid_char(*c)) {
            return Err(InvalidExtraGroupName::InvalidCharacter { name, character });
        }
        Ok(Self(name))
    }

    /// Returns the name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the name and returns the inner `String`.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for ExtraGroupName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for ExtraGroupName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for ExtraGroupName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl FromStr for ExtraGroupName {
    type Err = InvalidExtraGroupName;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl TryFrom<String> for ExtraGroupName {
    type Error = InvalidExtraGroupName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for ExtraGroupName {
    type Error = InvalidExtraGroupName;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl Serialize for ExtraGroupName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ExtraGroupName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Self::new(name).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_names() {
        for name in [
            "test",
            "cuda",
            "blas_openblas",
            "a",
            "x.y+z-1",
            "_",
            &"a".repeat(64),
        ] {
            assert!(ExtraGroupName::new(name).is_ok(), "should accept {name:?}");
        }
    }

    #[test]
    fn rejects_invalid_names() {
        assert!(matches!(
            ExtraGroupName::new(""),
            Err(InvalidExtraGroupName::InvalidLength { .. })
        ));
        assert!(matches!(
            ExtraGroupName::new("a".repeat(65)),
            Err(InvalidExtraGroupName::InvalidLength { .. })
        ));
        for bad in ["Test", "with space", "café", "a/b", "a:b"] {
            assert!(
                matches!(
                    ExtraGroupName::new(bad),
                    Err(InvalidExtraGroupName::InvalidCharacter { .. })
                ),
                "should reject {bad:?}"
            );
        }
    }

    #[test]
    fn deserialize_validates() {
        assert!(serde_json::from_str::<ExtraGroupName>("\"test\"").is_ok());
        assert!(serde_json::from_str::<ExtraGroupName>("\"Bad\"").is_err());
    }
}
