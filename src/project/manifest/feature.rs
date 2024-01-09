use super::{Activation, PyPiRequirement, SystemRequirements, Target, TargetSelector};
use crate::project::manifest::target::Targets;
use crate::project::SpecType;
use crate::task::Task;
use crate::utils::spanned::PixiSpanned;
use indexmap::IndexMap;
use rattler_conda_types::{Channel, NamelessMatchSpec, PackageName, Platform};
use serde::de::Error;
use serde::{Deserialize, Deserializer};
use serde_with::{serde_as, DisplayFromStr, PickFirst};
use std::collections::HashMap;

/// The name of a feature. This is either a string or default for the default feature.
#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub enum FeatureName {
    Default,
    Named(String),
}

impl<'de> Deserialize<'de> for FeatureName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match String::deserialize(deserializer)?.as_str() {
            "default" => Err(D::Error::custom(
                "The name 'default' is reserved for the default feature",
            )),
            name => Ok(FeatureName::Named(name.to_string())),
        }
    }
}

impl FeatureName {
    /// Returns the name of the feature or `None` if this is the default feature.
    pub fn name(&self) -> Option<&str> {
        match self {
            FeatureName::Default => None,
            FeatureName::Named(name) => Some(name),
        }
    }
}

/// A feature describes a set of functionalities. It allows us to group functionality and its
/// dependencies together.
///
/// Individual features cannot be used directly, instead they are grouped together into
/// environments. Environments are then locked and installed.
#[derive(Debug, Clone)]
pub struct Feature {
    /// The name of the feature or `None` if the feature is the default feature.
    pub name: FeatureName,

    /// The platforms this feature is available on.
    ///
    /// This value is `None` if this feature does not specify any platforms and the default
    /// platforms from the project should be used.
    pub platforms: Option<PixiSpanned<Vec<Platform>>>,

    /// Channels specific to this feature.
    ///
    /// This value is `None` if this feature does not specify any channels and the default
    /// channels from the project should be used.
    pub channels: Option<Vec<Channel>>,

    /// Additional system requirements
    pub system_requirements: SystemRequirements,

    /// Target specific configuration.
    pub targets: Targets,
}

impl<'de> Deserialize<'de> for Feature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[serde_as]
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields, rename_all = "kebab-case")]
        struct FeatureInner {
            #[serde(default)]
            platforms: Option<PixiSpanned<Vec<Platform>>>,
            #[serde_as(deserialize_as = "Option<Vec<super::serde::ChannelStr>>")]
            channels: Option<Vec<Channel>>,
            #[serde(default)]
            system_requirements: SystemRequirements,
            #[serde(default)]
            target: IndexMap<PixiSpanned<TargetSelector>, Target>,

            #[serde(default)]
            #[serde_as(as = "IndexMap<_, PickFirst<(DisplayFromStr, _)>>")]
            dependencies: IndexMap<PackageName, NamelessMatchSpec>,

            #[serde(default)]
            #[serde_as(as = "Option<IndexMap<_, PickFirst<(DisplayFromStr, _)>>>")]
            host_dependencies: Option<IndexMap<PackageName, NamelessMatchSpec>>,

            #[serde(default)]
            #[serde_as(as = "Option<IndexMap<_, PickFirst<(DisplayFromStr, _)>>>")]
            build_dependencies: Option<IndexMap<PackageName, NamelessMatchSpec>>,

            #[serde(default)]
            pypi_dependencies: Option<IndexMap<rip::types::PackageName, PyPiRequirement>>,

            /// Additional information to activate an environment.
            #[serde(default)]
            activation: Option<Activation>,

            /// Target specific tasks to run in the environment
            #[serde(default)]
            tasks: HashMap<String, Task>,
        }

        let inner = FeatureInner::deserialize(deserializer)?;

        let mut dependencies = HashMap::from_iter([(SpecType::Run, inner.dependencies)]);
        if let Some(host_deps) = inner.host_dependencies {
            dependencies.insert(SpecType::Host, host_deps);
        }
        if let Some(build_deps) = inner.build_dependencies {
            dependencies.insert(SpecType::Build, build_deps);
        }

        let default_target = Target {
            dependencies,
            pypi_dependencies: inner.pypi_dependencies,
            activation: inner.activation,
            tasks: inner.tasks,
        };

        Ok(Feature {
            name: FeatureName::Default,
            platforms: inner.platforms,
            channels: inner.channels,
            system_requirements: inner.system_requirements,
            targets: Targets::from_default_and_user_defined(default_target, inner.target),
        })
    }
}
