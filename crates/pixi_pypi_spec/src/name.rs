use pep508_rs::{InvalidNameError, PackageName};
use serde::{Serialize, Serializer};
use std::{borrow::Borrow, str::FromStr};

/// A package name for Pypi that also stores the source version of the name.
#[derive(Debug, Clone)]
pub struct PypiPackageName {
    source: String,
    normalized: PackageName,
}

impl PartialEq for PypiPackageName {
    fn eq(&self, other: &Self) -> bool {
        self.normalized == other.normalized
    }
}

impl Eq for PypiPackageName {}

impl std::hash::Hash for PypiPackageName {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.normalized.hash(state);
    }
}

impl Borrow<PackageName> for PypiPackageName {
    fn borrow(&self) -> &PackageName {
        &self.normalized
    }
}

impl FromStr for PypiPackageName {
    type Err = InvalidNameError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            source: name.to_string(),
            normalized: PackageName::from_str(name)?,
        })
    }
}

impl Serialize for PypiPackageName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.as_source().serialize(serializer)
    }
}

impl PypiPackageName {
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

#[cfg(test)]
mod tests {
    use std::{
        hash::{Hash, Hasher},
        str::FromStr,
    };

    use crate::PypiPackageName;

    #[test]
    fn hash_equality() {
        // Assert equality of two package names with different source strings
        let name = PypiPackageName::from_str("foo-bar").unwrap();
        let name2 = PypiPackageName::from_str("foo_bar").unwrap();
        assert_eq!(name, name2);

        // Assert that the hash values are equal
        let mut hasher = std::hash::DefaultHasher::new();
        name.hash(&mut hasher);
        let hash = hasher.finish();

        let mut hasher = std::hash::DefaultHasher::new();
        name2.hash(&mut hasher);
        let hash2 = hasher.finish();

        assert_eq!(
            hash, hash2,
            "Normalized PyPI name and Non-Normalized name do not hash to the same value"
        );
    }
}
