use pep508_rs::{InvalidNameError, PackageName};
use std::{borrow::Borrow, str::FromStr};

/// A package name for Pypi that also stores the source version of the name.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct PypiPackageName {
    source: String,
    normalized: PackageName,
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
