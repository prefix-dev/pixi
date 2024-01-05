use super::SystemRequirements;
use crate::project::manifest::target::Targets;
use crate::utils::spanned::PixiSpanned;
use rattler_conda_types::{Channel, Platform};
use serde::de::Error;
use serde::Deserialize;

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
