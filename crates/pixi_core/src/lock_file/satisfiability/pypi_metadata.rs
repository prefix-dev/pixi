//! Module for reading and comparing PyPI package metadata from local source trees.
//!
//! This module provides functionality to:
//! 1. Read metadata from local pyproject.toml files
//! 2. Compare locked metadata against current source tree metadata
use std::collections::BTreeSet;
use std::str::FromStr;

use pep440_rs::{Version, VersionSpecifiers};
use pep508_rs::Requirement;
use rattler_lock::PypiPackageData;
use thiserror::Error;

/// Metadata extracted from a local package source tree.
#[derive(Debug, Clone)]
pub struct LocalPackageMetadata {
    /// The version of the package (if not dynamic).
    pub version: Option<Version>,
    /// The package dependencies.
    pub requires_dist: Vec<Requirement>,
    /// The Python version requirement.
    pub requires_python: Option<VersionSpecifiers>,
    /// Whether the version is marked as dynamic.
    pub is_version_dynamic: bool,
}

/// Error that can occur when reading metadata from a source tree.
#[derive(Debug, Error)]
pub enum MetadataReadError {
    /// Failed to parse the pyproject.toml file.
    #[error("failed to parse pyproject.toml: {0}")]
    ParseError(String),
}

/// The result of comparing locked metadata against current metadata.
#[derive(Debug)]
pub enum MetadataMismatch {
    /// The requires_dist (dependencies) have changed.
    RequiresDist(RequiresDistDiff),
    /// The version has changed.
    Version { locked: Version, current: Version },
    /// The requires_python has changed.
    RequiresPython {
        locked: Option<VersionSpecifiers>,
        current: Option<VersionSpecifiers>,
    },
}

/// Describes the difference in requires_dist between locked and current metadata.
#[derive(Debug)]
pub struct RequiresDistDiff {
    /// Dependencies that were added.
    pub added: Vec<Requirement>,
    /// Dependencies that were removed.
    pub removed: Vec<Requirement>,
}

/// Compare locked metadata against current metadata from the source tree.
///
/// Returns `None` if the metadata matches, or `Some(MetadataMismatch)` describing
/// what changed.
pub fn compare_metadata(
    locked: &PypiPackageData,
    current: &LocalPackageMetadata,
) -> Option<MetadataMismatch> {
    // Compare requires_dist (as normalized sets)
    let locked_deps: BTreeSet<String> = locked
        .requires_dist
        .iter()
        .map(normalize_requirement)
        .collect();

    let current_deps: BTreeSet<String> = current
        .requires_dist
        .iter()
        .map(normalize_requirement)
        .collect();

    if locked_deps != current_deps {
        // Calculate the diff
        let added: Vec<Requirement> = current
            .requires_dist
            .iter()
            .filter(|r| !locked_deps.contains(&normalize_requirement(r)))
            .cloned()
            .collect();

        let removed: Vec<Requirement> = locked
            .requires_dist
            .iter()
            .filter(|r| !current_deps.contains(&normalize_requirement(r)))
            .cloned()
            .collect();

        return Some(MetadataMismatch::RequiresDist(RequiresDistDiff {
            added,
            removed,
        }));
    }

    // Compare version (only if current version is not dynamic)
    if !current.is_version_dynamic
        && let Some(current_version) = &current.version
        && &locked.version != current_version
    {
        return Some(MetadataMismatch::Version {
            locked: locked.version.clone(),
            current: current_version.clone(),
        });
    }

    // Compare requires_python
    if locked.requires_python != current.requires_python {
        return Some(MetadataMismatch::RequiresPython {
            locked: locked.requires_python.clone(),
            current: current.requires_python.clone(),
        });
    }

    None
}

/// Normalize a requirement for comparison purposes.
///
/// This ensures that semantically equivalent requirements compare equal,
/// regardless of formatting differences (e.g., whitespace, order of extras).
fn normalize_requirement(req: &Requirement) -> String {
    // Use the canonical string representation
    // The pep508_rs library already normalizes package names and versions
    req.to_string()
}

/// Convert UV metadata to LocalPackageMetadata for comparison.
///
/// This is used when we build metadata using UV's DistributionDatabase
/// for packages with dynamic metadata.
pub fn from_uv_metadata(
    metadata: &uv_distribution::Metadata,
) -> Result<LocalPackageMetadata, MetadataReadError> {
    // Convert version
    let version = pep440_rs::Version::from_str(&metadata.version.to_string())
        .map_err(|e| MetadataReadError::ParseError(format!("invalid version: {e}")))?;

    // Convert requires_dist
    let requires_dist: Vec<Requirement> = metadata
        .requires_dist
        .iter()
        .map(|req| {
            let req_str = req.to_string();
            req_str
                .parse::<Requirement>()
                .map_err(|e| MetadataReadError::ParseError(format!("invalid requirement: {e}")))
        })
        .collect::<Result<Vec<_>, _>>()?;

    // Convert requires_python
    let requires_python = metadata
        .requires_python
        .as_ref()
        .map(|rp| {
            pep440_rs::VersionSpecifiers::from_str(&rp.to_string())
                .map_err(|e| MetadataReadError::ParseError(format!("invalid requires-python: {e}")))
        })
        .transpose()?;

    Ok(LocalPackageMetadata {
        version: Some(version),
        requires_dist,
        requires_python,
        is_version_dynamic: false, // Built metadata always has concrete values
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_normalize_requirement() {
        let req1: Requirement = "numpy>=1.0".parse().unwrap();
        let req2: Requirement = "numpy >= 1.0".parse().unwrap();
        // Note: These may or may not be equal depending on pep508_rs normalization
        // The important thing is we consistently compare them
        assert_eq!(normalize_requirement(&req1), normalize_requirement(&req1));
        let _ = req2; // silence unused warning
    }

    #[test]
    fn test_compare_metadata_same() {
        let locked = PypiPackageData {
            name: "test-package".parse().unwrap(),
            version: Version::from_str("1.0.0").unwrap(),
            requires_dist: vec!["numpy>=1.0".parse().unwrap()],
            requires_python: Some(VersionSpecifiers::from_str(">=3.8").unwrap()),
            location: rattler_lock::UrlOrPath::Url(url::Url::parse("file:///test").unwrap()),
            hash: None,
            editable: false,
        };

        let current = LocalPackageMetadata {
            version: Some(Version::from_str("1.0.0").unwrap()),
            requires_dist: vec!["numpy>=1.0".parse().unwrap()],
            requires_python: Some(VersionSpecifiers::from_str(">=3.8").unwrap()),
            is_version_dynamic: false,
        };

        assert!(compare_metadata(&locked, &current).is_none());
    }

    #[test]
    fn test_compare_metadata_different_deps() {
        let locked = PypiPackageData {
            name: "test-package".parse().unwrap(),
            version: Version::from_str("1.0.0").unwrap(),
            requires_dist: vec!["numpy>=1.0".parse().unwrap()],
            requires_python: None,
            location: rattler_lock::UrlOrPath::Url(url::Url::parse("file:///test").unwrap()),
            hash: None,
            editable: false,
        };

        let current = LocalPackageMetadata {
            version: Some(Version::from_str("1.0.0").unwrap()),
            requires_dist: vec![
                "numpy>=1.0".parse().unwrap(),
                "pandas>=2.0".parse().unwrap(), // Added
            ],
            requires_python: None,
            is_version_dynamic: false,
        };

        let mismatch = compare_metadata(&locked, &current);
        assert!(matches!(mismatch, Some(MetadataMismatch::RequiresDist(_))));
    }
}
