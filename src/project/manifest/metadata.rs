use crate::project::manifest::python::PyPiIndex;
use crate::utils::spanned::PixiSpanned;
use indexmap::IndexMap;
use rattler_conda_types::{Platform, Version};
use serde::Deserialize;
use serde_with::{serde_as, DisplayFromStr};
use std::path::PathBuf;
use url::Url;

/// Describes the contents of the `[package]` section of the project manifest.
#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ProjectMetadata {
    /// The name of the project
    pub name: String,

    /// The version of the project
    #[serde_as(as = "Option<DisplayFromStr>")]
    pub version: Option<Version>,

    /// An optional project description
    pub description: Option<String>,

    /// Optional authors
    #[serde(default)]
    pub authors: Vec<String>,

    /// Python indices used by this project
    pub pypi_indices: Option<IndexMap<String, PyPiIndex>>,

    /// The channels used by the project
    #[serde_as(as = "Vec<super::channel::TomlPrioritizedChannelStrOrMap>")]
    pub channels: Vec<super::channel::PrioritizedChannel>,

    /// The platforms this project supports
    // TODO: This is actually slightly different from the rattler_conda_types::Platform because it
    //     should not include noarch.
    pub platforms: PixiSpanned<Vec<Platform>>,

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
