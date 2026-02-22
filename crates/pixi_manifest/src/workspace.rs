use std::{collections::HashMap, path::PathBuf};

use indexmap::IndexSet;
use pixi_toml::TomlEnum;
use rattler_conda_types::{NamedChannelOrUrl, Platform, Version, VersionSpec};
use serde::Deserialize;
use toml_span::{DeserError, Value};
use url::Url;

use super::pypi::pypi_options::PypiOptions;
use crate::{
    PrioritizedChannel, S3Options, Targets, exclude_newer::ExcludeNewer, preview::Preview,
};
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

    /// Solve strategy for the whole project.
    pub solve_strategy: Option<SolveStrategy>,

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

    /// Build variants defined directly in the manifest.
    pub build_variants: Targets<Option<HashMap<String, Vec<String>>>>,

    /// Ordered list of external variant configuration files.
    pub build_variant_files: Vec<BuildVariantSource>,

    /// Version requirement for pixi itself
    pub requires_pixi: Option<VersionSpec>,

    /// Exclude package candidates that are newer than this date.
    pub exclude_newer: Option<ExcludeNewer>,
}

/// A source that contributes additional build variant definitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildVariantSource {
    /// Load variants from a file relative to the workspace root.
    File(PathBuf),
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
pub enum SolveStrategy {
    #[default]
    Highest,
    Lowest,
    LowestDirect,
}

impl<'de> toml_span::Deserialize<'de> for SolveStrategy {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        TomlEnum::deserialize(value).map(TomlEnum::into_inner)
    }
}

impl From<SolveStrategy> for rattler_solve::SolveStrategy {
    fn from(value: SolveStrategy) -> Self {
        match value {
            SolveStrategy::Highest => rattler_solve::SolveStrategy::Highest,
            SolveStrategy::Lowest => rattler_solve::SolveStrategy::LowestVersion,
            SolveStrategy::LowestDirect => rattler_solve::SolveStrategy::LowestVersionDirect,
        }
    }
}

impl From<rattler_solve::SolveStrategy> for SolveStrategy {
    fn from(value: rattler_solve::SolveStrategy) -> Self {
        match value {
            rattler_solve::SolveStrategy::Highest => Self::Highest,
            rattler_solve::SolveStrategy::LowestVersion => Self::Lowest,
            rattler_solve::SolveStrategy::LowestVersionDirect => Self::LowestDirect,
        }
    }
}
