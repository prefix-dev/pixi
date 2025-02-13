//! Defines the build section for the pixi manifest.

use indexmap::IndexMap;
use pixi_spec::BinarySpec;
use rattler_conda_types::NamedChannelOrUrl;

use crate::toml::FromTomlStr;
use crate::{toml::TomlPackageBuild, TomlError};

/// A build section in the pixi manifest.
/// that defines what backend is used to build the project.
#[derive(Debug, Clone)]
pub struct PackageBuild {
    /// Information about the build backend
    pub backend: BuildBackend,

    /// Additional dependencies that should be installed alongside the backend.
    pub additional_dependencies: IndexMap<rattler_conda_types::PackageName, BinarySpec>,

    /// The channels to use for fetching build tools. If this is `None` the
    /// channels from the containing workspace should be used.
    pub channels: Option<Vec<NamedChannelOrUrl>>,

    /// Additional configuration for the build backend.
    pub configuration: Option<serde_value::Value>,
}

#[derive(Debug, Clone)]
pub struct BuildBackend {
    /// The name of the build backend to install
    pub name: rattler_conda_types::PackageName,

    /// The spec for the backend
    pub spec: BinarySpec,
}

impl PackageBuild {
    /// Parses the specified string as a toml representation of a build system.
    pub fn from_toml_str(source: &str) -> Result<Self, TomlError> {
        TomlPackageBuild::from_toml_str(source).and_then(TomlPackageBuild::into_build_system)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_build() {
        let toml = r#"
            backend = { name = "pixi-build-python", version = "12.*" }
            "#;

        let build = PackageBuild::from_toml_str(toml).unwrap();
        assert_eq!(build.backend.name.as_source(), "pixi-build-python");
    }
}
