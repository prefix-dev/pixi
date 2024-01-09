use crate::consts;
use crate::utils::spanned::PixiSpanned;
use serde::{self, Deserialize, Deserializer};
use std::borrow::Borrow;
use std::hash::{Hash, Hasher};

/// The name of an environment. This is either a string or default for the default environment.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EnvironmentName {
    Default,
    Named(String),
}

impl Hash for EnvironmentName {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state)
    }
}

impl EnvironmentName {
    /// Returns the name of the environment. This is either the name of the environment or the name
    /// of the default environment.
    pub fn as_str(&self) -> &str {
        match self {
            EnvironmentName::Default => consts::DEFAULT_ENVIRONMENT_NAME,
            EnvironmentName::Named(name) => name.as_str(),
        }
    }
}

impl Borrow<str> for EnvironmentName {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl<'de> Deserialize<'de> for EnvironmentName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match String::deserialize(deserializer)? {
            name if name == consts::DEFAULT_ENVIRONMENT_NAME => Ok(EnvironmentName::Default),
            name => Ok(EnvironmentName::Named(name)),
        }
    }
}

/// An environment describes a set of features that are available together.
///
/// Individual features cannot be used directly, instead they are grouped together into
/// environments. Environments are then locked and installed.
#[derive(Debug, Clone)]
pub struct Environment {
    /// The name of the environment
    pub name: EnvironmentName,

    /// The names of the features that together make up this environment.
    ///
    /// Note that the default feature is always added to the set of features that make up the
    /// environment.
    pub features: Vec<String>,

    /// The optional location of where the features of the environment are defined in the manifest toml.
    pub features_source_loc: Option<std::ops::Range<usize>>,

    /// An optional solver-group. Multiple environments can share the same solve-group. All the
    /// dependencies of the environment that share the same solve-group will be solved together.
    pub solve_group: Option<String>,
}

/// Helper struct to deserialize the environment from TOML.
/// The environment description can only hold these values.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(super) struct TomlEnvironment {
    pub features: PixiSpanned<Vec<String>>,
    pub solve_group: Option<String>,
}

pub(super) enum TomlEnvironmentMapOrSeq {
    Map(TomlEnvironment),
    Seq(Vec<String>),
}
impl TomlEnvironmentMapOrSeq {
    pub fn into_environment(self, name: EnvironmentName) -> Environment {
        match self {
            TomlEnvironmentMapOrSeq::Map(TomlEnvironment {
                features,
                solve_group,
            }) => Environment {
                name,
                features: features.value,
                features_source_loc: features.span,
                solve_group,
            },
            TomlEnvironmentMapOrSeq::Seq(features) => Environment {
                name,
                features,
                features_source_loc: None,
                solve_group: None,
            },
        }
    }
}
impl<'de> Deserialize<'de> for TomlEnvironmentMapOrSeq {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        serde_untagged::UntaggedEnumVisitor::new()
            .map(|map| map.deserialize().map(TomlEnvironmentMapOrSeq::Map))
            .seq(|seq| seq.deserialize().map(TomlEnvironmentMapOrSeq::Seq))
            .expecting("either a map or a sequence")
            .deserialize(deserializer)
    }
}
