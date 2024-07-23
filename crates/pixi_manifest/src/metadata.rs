use super::pypi::pypi_options::PypiOptions;
use crate::utils::PixiSpanned;
use indexmap::IndexSet;
use rattler_conda_types::{Platform, Version};
use rattler_solve::ChannelPriority;
use serde::Deserialize;
use serde_with::{serde_as, DisplayFromStr};
use std::{collections::HashMap, path::PathBuf};
use url::Url;

/// Describes the contents of the `[package]` section of the project manifest.
#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ProjectMetadata {
    /// The name of the project
    pub name: Option<String>, // set as optional to handle conversion from pyproject.toml

    /// The version of the project
    #[serde_as(as = "Option<DisplayFromStr>")]
    pub version: Option<Version>,

    /// An optional project description
    pub description: Option<String>,

    /// Optional authors
    #[serde(default)]
    pub authors: Vec<String>,

    /// The channels used by the project
    #[serde_as(as = "IndexSet<super::channel::TomlPrioritizedChannelStrOrMap>")]
    pub channels: IndexSet<super::channel::PrioritizedChannel>,

    /// Channel priority for the whole project
    #[serde(default)]
    pub channel_priority: Option<ChannelPriority>,

    /// The platforms this project supports
    // TODO: This is actually slightly different from the rattler_conda_types::Platform because it
    //     should not include noarch.
    pub platforms: PixiSpanned<IndexSet<Platform>>,

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

    /// URL or Path of the conda to pypi name mapping
    pub conda_pypi_map: Option<HashMap<String, String>>,

    /// The pypi options supported in the project
    pub pypi_options: Option<PypiOptions>,
}
