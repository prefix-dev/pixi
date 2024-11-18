//! Defines the build section for the pixi manifest.
use rattler_conda_types::Channel;
use rattler_conda_types::ChannelConfig;
use rattler_conda_types::MatchSpec;
use rattler_conda_types::NamedChannelOrUrl;
use rattler_conda_types::ParseChannelError;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use serde_with::DisplayFromStr;
use url::Url;

/// A build section in the pixi manifest.
/// that defines what backend is used to build the project.
#[serde_as]
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct BuildSection {
    /// The dependencies for the build tools which will be installed in the build environment.
    /// These need to be conda packages
    #[serde_as(as = "Vec<DisplayFromStr>")]
    pub dependencies: Vec<MatchSpec>,

    /// The command to start the build backend
    pub build_backend: String,

    /// The channels to use for fetching build tools
    pub channels: Vec<NamedChannelOrUrl>,
}

impl BuildSection {
    /// Returns the channels as URLs
    pub fn channels_url(&self, config: &ChannelConfig) -> Result<Vec<Url>, ParseChannelError> {
        self.channels
            .iter()
            .map(|c| c.clone().into_base_url(config))
            .collect()
    }

    /// Returns the channels as `Channel`s
    pub fn channels(&self, config: &ChannelConfig) -> Result<Vec<Channel>, ParseChannelError> {
        self.channels
            .iter()
            .map(|c| c.clone().into_channel(config))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_build() {
        let toml = r#"
            dependencies = ["pixi-build-python > 12"]
            build-backend = "pixi-build-python"
            "#;

        let build: BuildSection = toml_edit::de::from_str(toml).unwrap();
        assert_eq!(build.dependencies.len(), 1);
        assert_eq!(
            build.dependencies[0].to_string(),
            "pixi-build-python >12".to_string()
        );
    }
}
