use pep440_rs::VersionSpecifiers;
use pep508_rs::{InvalidNameError, PackageName};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{Display, Formatter, Write};
use std::{borrow::Borrow, str::FromStr};

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

impl<'de> Deserialize<'de> for PyPiPackageName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        serde_untagged::UntaggedEnumVisitor::new()
            .string(|str| PyPiPackageName::from_str(str).map_err(serde::de::Error::custom))
            .expecting("a string")
            .deserialize(deserializer)
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
            VersionOrStar::Star => VersionSpecifiers::empty(),
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

#[derive(Debug, Serialize, Clone, PartialEq, Eq, Hash)]
#[serde(untagged, rename_all = "snake_case", deny_unknown_fields)]
pub enum GitRev {
    Short(String),
    Full(String),
}

impl GitRev {
    pub fn as_full(&self) -> Option<&str> {
        match self {
            GitRev::Full(full) => Some(full.as_str()),
            GitRev::Short(_) => None,
        }
    }
}

#[derive(thiserror::Error, Clone, Debug, Eq, PartialEq)]
pub enum GitRevParseError {
    #[error("Invalid length must be less than 40, actual size: {0}")]
    InvalidLength(usize),
    #[error("Found invalid characters for git revision {0}")]
    InvalidCharacters(String),
}

impl FromStr for GitRev {
    type Err = GitRevParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if !s.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(GitRevParseError::InvalidCharacters(s.to_string()));
        }

        // Parse the git revision
        match s.len() {
            0..=39 => Ok(GitRev::Short(s.to_string())),
            40 => Ok(GitRev::Full(s.to_string())),
            _ => Err(GitRevParseError::InvalidLength(s.len())),
        }
    }
}

impl Display for GitRev {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            GitRev::Short(s) => f.write_str(s),
            GitRev::Full(s) => f.write_str(s),
        }
    }
}

impl<'de> Deserialize<'de> for GitRev {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: String = Deserialize::deserialize(deserializer)?;
        if s.len() == 40 {
            Ok(GitRev::Full(s))
        } else {
            Ok(GitRev::Short(s))
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_git_rev_from_str_valid_short() {
        let rev = GitRev::from_str("abc123").unwrap();
        assert_eq!(rev, GitRev::Short("abc123".to_string()));
    }

    #[test]
    fn test_git_rev_from_str_valid_full() {
        let rev = GitRev::from_str("0123456789abcdef0123456789abcdef01234567").unwrap();
        assert_eq!(
            rev,
            GitRev::Full("0123456789abcdef0123456789abcdef01234567".to_string())
        );
    }

    #[test]
    fn test_git_rev_from_str_invalid_characters() {
        let rev = GitRev::from_str("\x1b");
        assert!(rev.is_err());
        assert_eq!(
            rev.unwrap_err(),
            GitRevParseError::InvalidCharacters("\x1b".to_string())
        );
    }

    #[test]
    fn test_git_rev_from_str_invalid_length() {
        let rev = GitRev::from_str("0123456789abcdef0123456789abcdef0123456789");
        assert!(rev.is_err());
        assert_eq!(rev.unwrap_err(), GitRevParseError::InvalidLength(42));
    }
}
