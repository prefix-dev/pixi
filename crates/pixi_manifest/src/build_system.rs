//! Defines the build section for the pixi manifest.

use indexmap::IndexMap;
use pixi_spec::BinarySpec;
use rattler_conda_types::NamedChannelOrUrl;

use crate::toml::FromTomlStr;
use crate::{toml::TomlBuildSystem, TomlError};

/// A build section in the pixi manifest.
/// that defines what backend is used to build the project.
#[derive(Debug, Clone)]
pub struct BuildSystem {
    /// Information about the build backend
    pub build_backend: BuildBackend,

    /// Additional dependencies that should be installed alongside the backend.
    pub additional_dependencies: IndexMap<rattler_conda_types::PackageName, BinarySpec>,

    /// The channels to use for fetching build tools. If this is `None` the
    /// channels from the containing workspace should be used.
    pub channels: Option<Vec<NamedChannelOrUrl>>,
}

#[derive(Debug, Clone)]
pub struct BuildBackend {
    /// The name of the build backend to install
    pub name: rattler_conda_types::PackageName,

    /// The spec for the backend
    pub spec: BinarySpec,

    /// Additional arguments to pass to the build backend. In the manifest these are read from the
    /// `[build-backend]` section.
    pub additional_args: Option<serde_value::Value>,
}

impl BuildSystem {
    /// Parses the specified string as a toml representation of a build system.
    pub fn from_toml_str(source: &str) -> Result<Self, TomlError> {
        TomlBuildSystem::from_toml_str(source).and_then(TomlBuildSystem::into_build_system)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_build() {
        let toml = r#"
            build-backend = { name = "pixi-build-python", version = "12.*" }
            "#;

        let build = BuildSystem::from_toml_str(toml).unwrap();
        assert_eq!(build.build_backend.name.as_source(), "pixi-build-python");
    }
}
