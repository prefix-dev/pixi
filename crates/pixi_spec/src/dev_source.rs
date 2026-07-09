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
/// my-package = { path = "../my-package" }
/// ```
///
/// This would be represented as:
/// ```ignore
/// DevSourceSpec {
///     source: SourceLocationSpec::Path(PathSpec { path: "../my-package" }),
///     extras: vec![],
/// }
/// ```
///
/// A dev source may additionally request one or more extra-dependency groups
/// declared by the source package (`package.extra-dependencies.<name>`). When
/// requested, the dependencies of those groups are installed into the
/// environment alongside the regular build/host/run dependencies:
///
/// ```toml
/// [dev]
/// my-package = { path = "../my-package", extras = ["test"] }
/// ```
#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize)]
pub struct DevSourceSpec {
    /// The source location (path/git/url)
    pub source: SourceLocationSpec,

    /// The extra-dependency groups to include from the source package. Empty
    /// when no extras were requested.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extras: Vec<String>,
}
