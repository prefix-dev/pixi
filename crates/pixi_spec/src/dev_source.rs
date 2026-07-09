//! Development source specifications.
//!
//! This module defines types for specifying development sources in pixi manifests.
//! Development sources are source packages whose dependencies should be installed
//! without building the package itself - useful for development environments.

use crate::SourceLocationSpec;

/// A development source specification as provided by the user (e.g., from pixi.toml).
///
/// This represents a source package whose dependencies should be installed without
/// building the package itself. This is useful for development environments where you
/// want to work on a package while having its dependencies available.
///
/// The available outputs are discovered by querying the build backend metadata.
///
/// # Example
///
/// In `pixi.toml`:
/// ```toml
/// [dev]
/// my-package = { path = "../my-package", extras = ["test"] }
/// ```
///
/// This would be represented as:
/// ```ignore
/// DevSourceSpec {
///     source: SourceLocationSpec::Path(PathSpec { path: "../my-package" }),
///     extras: Some(vec!["test".to_string()]),
/// }
/// ```
#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize)]
pub struct DevSourceSpec {
    /// The source location (path/git/url)
    pub source: SourceLocationSpec,

    /// Optional extra dependency groups of the source package to include.
    ///
    /// Each name refers to a group declared by the package under
    /// `package.extra-dependencies.<name>`; the dependencies of the selected
    /// groups are installed alongside the package's build/host/run
    /// dependencies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extras: Option<Vec<String>>,
}
