//! Development source specifications.
//!
//! This module defines types for specifying development sources in pixi manifests.
//! Development sources are source packages whose dependencies should be installed
//! without building the package itself - useful for development environments.

use crate::SourceSpec;

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
/// [develop]
/// my-package = { path = "../my-package" }
/// ```
///
/// This would be represented as:
/// ```ignore
/// DevSourceSpec {
///     source: SourceSpec::Path(PathSourceSpec { path: "../my-package" }),
/// }
/// ```
#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize)]
pub struct DevSourceSpec {
    /// The source specification (path/git/url)
    pub source: SourceSpec,
}
