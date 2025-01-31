use std::{
    borrow::Borrow,
    fmt::{Display, Formatter, Write},
    str::FromStr,
};

use pep440_rs::VersionSpecifiers;
use pep508_rs::{InvalidNameError, PackageName};
use pixi_toml::TomlFromStr;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use toml_span::{DeserError, Value};

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
/// A package name for PyPI that also stores the source version of the name.
pub struct PyPiPackageName {
    source: String,
    normalized: PackageName,
}

impl Borrow<PackageName> for PyPiPackageName {
    fn borrow(&self) -> &PackageName {
        &self.normalized
    }
}

impl FromStr for PyPiPackageName {
    type Err = InvalidNameError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            source: name.to_string(),
            normalized: PackageName::from_str(name)?,
        })
    }
}

impl PyPiPackageName {
    pub fn from_normalized(normalized: PackageName) -> Self {
        Self {
            source: normalized.to_string(),
            normalized,
        }
    }

    pub fn as_normalized(&self) -> &PackageName {
        &self.normalized
    }

    pub fn as_source(&self) -> &str {
        &self.source
    }
}

/// The pep crate does not support "*" as a version specifier, so we need to
/// handle it ourselves.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum VersionOrStar {
    Version(VersionSpecifiers),
    Star,
}

impl FromStr for VersionOrStar {
    type Err = pep440_rs::VersionSpecifiersParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "*" {
            Ok(VersionOrStar::Star)
        } else {
            Ok(VersionOrStar::Version(VersionSpecifiers::from_str(s)?))
        }
    }
}

impl<'de> toml_span::Deserialize<'de> for VersionOrStar {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        TomlFromStr::deserialize(value).map(TomlFromStr::into_inner)
    }
}

impl Display for VersionOrStar {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            VersionOrStar::Version(v) => f.write_str(&format!("{}", v)),
            VersionOrStar::Star => f.write_char('*'),
        }
    }
}

impl From<VersionOrStar> for Option<pep508_rs::VersionOrUrl> {
    fn from(val: VersionOrStar) -> Self {
        match val {
            VersionOrStar::Version(v) => Some(pep508_rs::VersionOrUrl::VersionSpecifier(v)),
            VersionOrStar::Star => None,
        }
    }
}

impl From<VersionOrStar> for VersionSpecifiers {
    fn from(value: VersionOrStar) -> Self {
        match value {
            VersionOrStar::Version(v) => v,
            VersionOrStar::Star => VersionSpecifiers::from_iter(vec![]),
        }
    }
}

impl Serialize for VersionOrStar {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for VersionOrStar {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        VersionOrStar::from_str(&s).map_err(serde::de::Error::custom)
    }
}
