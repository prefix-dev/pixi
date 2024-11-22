use std::{collections::HashMap, path::PathBuf};

use indexmap::IndexSet;
use rattler_conda_types::{NamedChannelOrUrl, Platform, Version};
use rattler_solve::ChannelPriority;
use serde::Deserialize;
use serde_with::{serde_as, DisplayFromStr};
use url::Url;

use crate::{
    preview::Preview, pypi::pypi_options::PypiOptions, utils::PixiSpanned, PrioritizedChannel,
};

/// The TOML representation of the `[[workspace]]` section in a pixi manifest.
#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct TomlWorkspace {
    // In TOML the workspace name can be empty. It is a required field though, but this is enforced
    // when converting the TOML model to the actual manifest. When using a PyProject we want to use
    // the name from the PyProject file.
    pub name: Option<String>,

    #[serde_as(as = "Option<DisplayFromStr>")]
    pub version: Option<Version>,
    pub description: Option<String>,
    pub authors: Option<Vec<String>>,
    #[serde_as(as = "IndexSet<super::TomlPrioritizedChannel>")]
    pub channels: IndexSet<PrioritizedChannel>,
    #[serde(default)]
    pub channel_priority: Option<ChannelPriority>,
    // TODO: This is actually slightly different from the rattler_conda_types::Platform because it
    //     should not include noarch.
    pub platforms: PixiSpanned<IndexSet<Platform>>,
    pub license: Option<String>,
    pub license_file: Option<PathBuf>,
    pub readme: Option<PathBuf>,
    pub homepage: Option<Url>,
    pub repository: Option<Url>,
    pub documentation: Option<Url>,
    pub conda_pypi_map: Option<HashMap<NamedChannelOrUrl, String>>,
    pub pypi_options: Option<PypiOptions>,

    #[serde(default)]
    pub preview: Preview,
}
