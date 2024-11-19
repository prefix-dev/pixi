//! Defines the build section for the pixi manifest.
use rattler_conda_types::MatchSpec;
use rattler_conda_types::NamedChannelOrUrl;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use serde_with::DisplayFromStr;

/// A build section in the pixi manifest.
/// that defines what backend is used to build the project.
#[serde_as]
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct BuildSystem {
    /// The dependencies for the build tools which will be installed in the build environment.
    /// These need to be conda packages
    #[serde_as(as = "Vec<DisplayFromStr>")]
    pub dependencies: Vec<MatchSpec>,

    /// The command to start the build backend
    pub build_backend: String,

    /// The channels to use for fetching build tools
    pub channels: Vec<NamedChannelOrUrl>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_build() {
        let toml = r#"
            channels = ["conda-forge"]
            dependencies = ["pixi-build-python > 12"]
            build-backend = "pixi-build-python"
            "#;

        let build: BuildSystem = toml_edit::de::from_str(toml).unwrap();
        assert_eq!(build.dependencies.len(), 1);
        assert_eq!(
            build.dependencies[0].to_string(),
            "pixi-build-python >12".to_string()
        );
    }
}
