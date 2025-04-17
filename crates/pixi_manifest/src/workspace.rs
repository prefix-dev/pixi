use std::{collections::HashMap, path::PathBuf};

use indexmap::IndexSet;
use pixi_toml::TomlEnum;
use rattler_conda_types::{NamedChannelOrUrl, Platform, Version, VersionSpec};
use serde::Deserialize;
use toml_span::{DeserError, Value};
use url::Url;

use super::pypi::pypi_options::PypiOptions;
use crate::exclude_newer::ExcludeNewer;
use crate::{preview::Preview, PrioritizedChannel, S3Options, Targets};
use minijinja::{AutoEscape, Environment, UndefinedBehavior};
use once_cell::sync::Lazy;

pub static JINJA_ENV: Lazy<Environment<'static>> = Lazy::new(|| {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.set_auto_escape_callback(|_| AutoEscape::None);
    env
});

/// Describes the contents of the `[workspace]` section of the project manifest.
#[derive(Debug, Default, Clone)]
pub struct Workspace {
    /// The name of the project
    pub name: Option<String>,

    /// The version of the project
    pub version: Option<Version>,

    /// An optional project description
    pub description: Option<String>,

    /// Optional authors
    pub authors: Option<Vec<String>>,

    /// The channels used by the project
    pub channels: IndexSet<PrioritizedChannel>,

    /// Channel priority for the whole project
    pub channel_priority: Option<ChannelPriority>,

    /// The platforms this project supports
    pub platforms: IndexSet<Platform>,

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
    pub conda_pypi_map: Option<HashMap<NamedChannelOrUrl, String>>,

    /// The pypi options supported in the project
    pub pypi_options: Option<PypiOptions>,

    /// The S3 options supported in the project
    pub s3_options: Option<HashMap<String, S3Options>>,

    /// Preview features
    pub preview: Preview,

    /// Build variants
    pub build_variants: Targets<Option<HashMap<String, Vec<String>>>>,

    /// Version requirement for pixi itself
    pub requires_pixi: Option<VersionSpec>,

    /// Exclude package candidates that are newer than this date.
    pub exclude_newer: Option<ExcludeNewer>,
}

#[derive(
    Debug,
    Copy,
    Clone,
    Default,
    Eq,
    PartialEq,
    strum::Display,
    strum::VariantNames,
    strum::EnumString,
    Deserialize,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum ChannelPriority {
    #[default]
    Strict,
    Disabled,
}

impl<'de> toml_span::Deserialize<'de> for ChannelPriority {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        TomlEnum::deserialize(value).map(TomlEnum::into_inner)
    }
}

impl From<ChannelPriority> for rattler_solve::ChannelPriority {
    fn from(value: ChannelPriority) -> Self {
        match value {
            ChannelPriority::Strict => rattler_solve::ChannelPriority::Strict,
            ChannelPriority::Disabled => rattler_solve::ChannelPriority::Disabled,
        }
    }
}

impl From<rattler_solve::ChannelPriority> for ChannelPriority {
    fn from(value: rattler_solve::ChannelPriority) -> Self {
        match value {
            rattler_solve::ChannelPriority::Strict => ChannelPriority::Strict,
            rattler_solve::ChannelPriority::Disabled => ChannelPriority::Disabled,
        }
    }
}
