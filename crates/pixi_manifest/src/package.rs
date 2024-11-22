use std::path::PathBuf;

use rattler_conda_types::Version;
use url::Url;

/// Defines the contents of the `[package]` section of the project manifest.
#[derive(Debug, Clone)]
pub struct Package {
    /// The name of the project
    pub name: String,

    /// The version of the project
    pub version: Version,

    /// An optional project description
    pub description: Option<String>,

    /// Optional authors
    pub authors: Option<Vec<String>>,

    /// The license as a valid SPDX string (e.g. MIT AND Apache-2.0)
    pub license: Option<String>,

    /// The license file (relative to the project root)
    pub license_file: Option<PathBuf>,

    /// Path to the README file of the project (relative to the project root)
    pub readme: Option<PathBuf>,

    /// URL of the project homepage
    pub homepage: Option<Url>,

    /// URL of the project source repository
    pub repository: Option<Url>,

    /// URL of the project documentation
    pub documentation: Option<Url>,
}
