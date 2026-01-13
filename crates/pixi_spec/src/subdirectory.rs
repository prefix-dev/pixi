//! A validated subdirectory path type.

use std::{
    fmt::{Display, Formatter},
    path::{Component, Path, PathBuf},
    str::FromStr,
};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// A validated subdirectory path.
///
/// This type ensures that the path:
/// - Is not absolute (doesn't start with `/`)
/// - Doesn't contain parent directory references (`..`)
///
/// This provides type safety and prevents path traversal attacks when
/// specifying subdirectories within git repositories or archives.
#[derive(Debug, Clone, Default, Hash, Eq, PartialEq, PartialOrd, Ord)]
pub struct Subdirectory(PathBuf);

/// Error returned when parsing an invalid subdirectory path.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SubdirectoryError {
    /// The path is absolute.
    #[error("subdirectory path cannot be absolute: {0}")]
    AbsolutePath(String),

    /// The path contains parent directory references.
    #[error("subdirectory path cannot contain '..': {0}")]
    ParentReference(String),
}

impl Subdirectory {
    /// Creates a new subdirectory from a path, validating it.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, SubdirectoryError> {
        let path = path.into();
        Self::validate(&path)?;
        Ok(Self(path))
    }

    /// Validates a path for use as a subdirectory.
    fn validate(path: &Path) -> Result<(), SubdirectoryError> {
        // Check if path is absolute
        if path.is_absolute() {
            return Err(SubdirectoryError::AbsolutePath(
                path.to_string_lossy().into_owned(),
            ));
        }

        // Check for parent directory references
        for component in path.components() {
            if matches!(component, Component::ParentDir) {
                return Err(SubdirectoryError::ParentReference(
                    path.to_string_lossy().into_owned(),
                ));
            }
        }

        Ok(())
    }

    /// Returns true if this subdirectory is empty (no path specified).
    pub fn is_empty(&self) -> bool {
        self.0.as_os_str().is_empty()
    }

    /// Returns the inner path as a reference.
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// Returns the inner PathBuf.
    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }

    /// Converts to an `Option<String>`, returning None if empty.
    pub fn to_option_string(&self) -> Option<String> {
        if self.is_empty() {
            None
        } else {
            Some(self.0.to_string_lossy().into_owned())
        }
    }
}

impl AsRef<Path> for Subdirectory {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl From<Subdirectory> for PathBuf {
    fn from(value: Subdirectory) -> Self {
        value.0
    }
}

impl TryFrom<PathBuf> for Subdirectory {
    type Error = SubdirectoryError;

    fn try_from(value: PathBuf) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<String> for Subdirectory {
    type Error = SubdirectoryError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(PathBuf::from(value))
    }
}

impl TryFrom<&str> for Subdirectory {
    type Error = SubdirectoryError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(PathBuf::from(value))
    }
}

impl TryFrom<Option<String>> for Subdirectory {
    type Error = SubdirectoryError;

    fn try_from(value: Option<String>) -> Result<Self, Self::Error> {
        match value {
            Some(s) => Self::new(PathBuf::from(s)),
            None => Ok(Self::default()),
        }
    }
}

impl From<Subdirectory> for Option<String> {
    fn from(value: Subdirectory) -> Self {
        value.to_option_string()
    }
}

impl FromStr for Subdirectory {
    type Err = SubdirectoryError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(PathBuf::from(s))
    }
}

impl Display for Subdirectory {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.display())
    }
}

impl Serialize for Subdirectory {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Serialize as a string
        self.0.to_string_lossy().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Subdirectory {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::new(PathBuf::from(&s)).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_subdirectories() {
        assert!(Subdirectory::new("foo").is_ok());
        assert!(Subdirectory::new("foo/bar").is_ok());
        assert!(Subdirectory::new("foo/bar/baz").is_ok());
        assert!(Subdirectory::new("").is_ok());
        assert!(Subdirectory::new(".").is_ok());
        assert!(Subdirectory::new("./foo").is_ok());
    }

    #[test]
    fn test_absolute_path_rejected() {
        let result = Subdirectory::new("/foo");
        assert!(matches!(result, Err(SubdirectoryError::AbsolutePath(_))));
    }

    #[test]
    fn test_parent_reference_rejected() {
        let result = Subdirectory::new("foo/../bar");
        assert!(matches!(result, Err(SubdirectoryError::ParentReference(_))));

        let result = Subdirectory::new("..");
        assert!(matches!(result, Err(SubdirectoryError::ParentReference(_))));

        let result = Subdirectory::new("../foo");
        assert!(matches!(result, Err(SubdirectoryError::ParentReference(_))));
    }

    #[test]
    fn test_is_empty() {
        assert!(Subdirectory::default().is_empty());
        assert!(Subdirectory::new("").unwrap().is_empty());
        assert!(!Subdirectory::new("foo").unwrap().is_empty());
    }

    #[test]
    fn test_serde_roundtrip() {
        let subdir = Subdirectory::new("foo/bar").unwrap();
        let json = serde_json::to_string(&subdir).unwrap();
        assert_eq!(json, "\"foo/bar\"");

        let deserialized: Subdirectory = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, subdir);
    }

    #[test]
    fn test_serde_rejects_invalid() {
        let result: Result<Subdirectory, _> = serde_json::from_str("\"/absolute\"");
        assert!(result.is_err());

        let result: Result<Subdirectory, _> = serde_json::from_str("\"../parent\"");
        assert!(result.is_err());
    }
}
