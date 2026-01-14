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
/// - Does not escape its root (e.g., `../foo` is invalid, but `a/../b` is valid)
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

    /// The path escapes its root directory.
    #[error("subdirectory path escapes its root: {0}")]
    EscapesRoot(String),
}

impl Subdirectory {
    /// Creates a new subdirectory from a path, validating and normalizing it.
    ///
    /// Normalization includes:
    /// - Removing `.` (current directory) components
    /// - Resolving `..` (parent directory) components where possible
    /// - Removing trailing slashes
    /// - Collapsing multiple slashes
    ///
    /// This ensures that equivalent paths like `"./foobar"`, `"foobar"`, and
    /// `"foobar/"` all result in the same normalized `Subdirectory`.
    ///
    /// Paths that would escape the root (e.g., `../foo` or `a/../../b`) are rejected.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, SubdirectoryError> {
        let path = path.into();
        Self::validate(&path)?;
        let normalized = Self::normalize(&path)?;
        Ok(Self(normalized))
    }

    /// Validates a path for use as a subdirectory.
    fn validate(path: &Path) -> Result<(), SubdirectoryError> {
        // Check if path is absolute.
        // On Windows, `is_absolute()` only returns true for paths like `C:\...`,
        // so we also need to check for Unix-style absolute paths starting with `/`.
        if path.is_absolute() || path.to_string_lossy().starts_with('/') {
            return Err(SubdirectoryError::AbsolutePath(
                path.to_string_lossy().into_owned(),
            ));
        }

        Ok(())
    }

    /// Normalizes a path by removing `.` components, resolving `..` components,
    /// and removing trailing slashes.
    ///
    /// Returns an error if the path would escape the root directory.
    pub fn normalize(path: &Path) -> Result<PathBuf, SubdirectoryError> {
        let mut normalized = PathBuf::new();
        for component in path.components() {
            match component {
                // Skip current directory components (`.`)
                Component::CurDir => {}
                // Handle parent directory components (`..`)
                Component::ParentDir => {
                    // If we can't pop (path is empty), the path escapes the root
                    if !normalized.pop() {
                        return Err(SubdirectoryError::EscapesRoot(
                            path.to_string_lossy().into_owned(),
                        ));
                    }
                }
                // Keep normal path segments
                Component::Normal(segment) => normalized.push(segment),
                // RootDir and Prefix shouldn't occur (we validate against absolute paths)
                _ => {}
            }
        }
        Ok(normalized)
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
        // Use forward slashes for consistent cross-platform display
        let path_str = self.0.to_string_lossy();
        write!(f, "{}", path_str.replace('\\', "/"))
    }
}

impl Serialize for Subdirectory {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Serialize with forward slashes for cross-platform consistency
        let path_str = self.0.to_string_lossy();
        path_str.replace('\\', "/").serialize(serializer)
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
    fn test_parent_reference_within_root_allowed() {
        // These should be valid because they don't escape the root
        let result = Subdirectory::new("foo/../bar").unwrap();
        assert_eq!(result.as_path(), Path::new("bar"));

        let result = Subdirectory::new("a/b/../c").unwrap();
        assert_eq!(result.as_path(), Path::new("a/c"));

        let result = Subdirectory::new("a/b/c/../../d").unwrap();
        assert_eq!(result.as_path(), Path::new("a/d"));

        // Going back to root should result in empty path
        let result = Subdirectory::new("a/..").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_escapes_root_rejected() {
        // These should be rejected because they escape the root
        let result = Subdirectory::new("..");
        assert!(matches!(result, Err(SubdirectoryError::EscapesRoot(_))));

        let result = Subdirectory::new("../foo");
        assert!(matches!(result, Err(SubdirectoryError::EscapesRoot(_))));

        let result = Subdirectory::new("a/../../b");
        assert!(matches!(result, Err(SubdirectoryError::EscapesRoot(_))));

        let result = Subdirectory::new("a/../..");
        assert!(matches!(result, Err(SubdirectoryError::EscapesRoot(_))));
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

    #[test]
    fn test_empty_path_equivalence() {
        // All of these should be equivalent representations of the current/empty directory
        let empty = Subdirectory::new("").unwrap();
        let dot_slash = Subdirectory::new("./").unwrap();
        let dot_multi_slash = Subdirectory::new(".///").unwrap();

        assert_eq!(empty, dot_slash);
        assert_eq!(empty, dot_multi_slash);
        assert_eq!(dot_slash, dot_multi_slash);
    }

    #[test]
    fn test_subdirectory_path_equivalence() {
        // All of these should be equivalent representations of the same subdirectory
        let plain = Subdirectory::new("foobar").unwrap();
        let with_dot_prefix = Subdirectory::new("./foobar").unwrap();
        let with_trailing_slash = Subdirectory::new("foobar/").unwrap();

        assert_eq!(plain, with_dot_prefix);
        assert_eq!(plain, with_trailing_slash);
        assert_eq!(with_dot_prefix, with_trailing_slash);
    }

    #[test]
    fn test_nested_path_equivalence() {
        // Nested paths with various representations
        let plain = Subdirectory::new("foo/bar/baz").unwrap();
        let with_dot_prefix = Subdirectory::new("./foo/bar/baz").unwrap();
        let with_trailing_slash = Subdirectory::new("foo/bar/baz/").unwrap();
        let with_inner_dots = Subdirectory::new("./foo/./bar/./baz").unwrap();

        assert_eq!(plain, with_dot_prefix);
        assert_eq!(plain, with_trailing_slash);
        assert_eq!(plain, with_inner_dots);
    }

    #[test]
    fn test_multiple_slashes_normalized() {
        // Multiple slashes should be collapsed
        let single = Subdirectory::new("foo/bar").unwrap();
        let double = Subdirectory::new("foo//bar").unwrap();
        let triple = Subdirectory::new("foo///bar").unwrap();

        assert_eq!(single, double);
        assert_eq!(single, triple);
    }

    #[test]
    fn test_just_dot_is_empty() {
        // A single `.` should normalize to empty
        let dot = Subdirectory::new(".").unwrap();
        let empty = Subdirectory::new("").unwrap();

        assert_eq!(dot, empty);
        assert!(dot.is_empty());
    }

    #[test]
    fn test_serialization_is_normalized() {
        // Paths should serialize to their normalized form
        let cases: Vec<(&str, Subdirectory)> = vec![
            ("empty", Subdirectory::new("").unwrap()),
            ("dot", Subdirectory::new(".").unwrap()),
            ("dot_slash", Subdirectory::new("./").unwrap()),
            ("simple", Subdirectory::new("foobar").unwrap()),
            ("simple_with_dot", Subdirectory::new("./foobar").unwrap()),
            (
                "simple_with_trailing",
                Subdirectory::new("foobar/").unwrap(),
            ),
            ("nested", Subdirectory::new("foo/bar").unwrap()),
            (
                "nested_with_dots",
                Subdirectory::new("./foo/./bar").unwrap(),
            ),
            (
                "nested_with_slashes",
                Subdirectory::new("foo//bar").unwrap(),
            ),
        ];

        // Show input → serialized output
        let snapshot: Vec<_> = cases
            .into_iter()
            .map(|(name, subdir)| {
                let serialized = serde_json::to_string(&subdir).unwrap();
                (name, serialized)
            })
            .collect();

        insta::assert_yaml_snapshot!(snapshot);
    }
}
