use indexmap::{IndexMap, IndexSet};
use pixi_manifest::PrioritizedChannel;
use rattler_conda_types::{NamedChannelOrUrl, PackageName, Platform};
use serde_with::{serde_as, serde_derive::Deserialize};
use uv_toolchain::platform;

use super::environment::EnvironmentName;

use super::errors::ManifestError;
use pixi_spec::PixiSpec;

/// Describes the contents of a parsed global project manifest.
#[serde_as]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ParsedManifest {
    /// The environments the project can create.
    #[serde(default, rename = "envs")]
    environments: IndexMap<EnvironmentName, ParsedEnvironment>,
}

impl ParsedManifest {
    /// Parses a toml string into a project manifest.
    pub(crate) fn from_toml_str(source: &str) -> Result<Self, ManifestError> {
        toml_edit::de::from_str(source).map_err(ManifestError::from)
    }

    pub(crate) fn environments(&self) -> IndexMap<EnvironmentName, ParsedEnvironment> {
        self.environments.clone()
    }
}

#[serde_as]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct ParsedEnvironment {
    #[serde_as(as = "IndexSet<pixi_manifest::TomlPrioritizedChannelStrOrMap>")]
    channels: IndexSet<pixi_manifest::PrioritizedChannel>,
    // Platform used by the environment.
    platform: Option<Platform>,
    #[serde(default, deserialize_with = "pixi_manifest::deserialize_package_map")]
    pub(crate) dependencies: IndexMap<PackageName, PixiSpec>,
    pub(crate) exposed: IndexMap<String, String>,
}

impl ParsedEnvironment {
    // If `self.platform` is `None` is not given, the current platform is used
    pub(crate) fn platform(&self) -> Platform {
        if let Some(platform) = self.platform {
            platform
        } else {
            Platform::current()
        }
    }

    /// Returns the channels associated with this collection.
    pub(crate) fn channels(&self) -> IndexSet<&NamedChannelOrUrl> {
        PrioritizedChannel::sort_channels_by_priority(&self.channels).collect()
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;

    use super::ParsedManifest;

    #[test]
    fn test_invalid_key() {
        let examples = [
            "[invalid]",
            "[envs.ipython.invalid]",
            "[envs.INVALID.dependencies]",
            "[envs.python_3.dependencies]",
        ];
        assert_snapshot!(examples
            .into_iter()
            .map(|example| ParsedManifest::from_toml_str(example)
                .unwrap_err()
                .to_string())
            .collect::<Vec<_>>()
            .join("\n"))
    }

    #[test]
    fn test_duplicate_exposed() {
        let contents = r#"
        [envs.python-3-10]
        channels = ["conda-forge"]
        [envs.python-3-10.dependencies]
        python = "3.10"
        [envs.python-3-10.exposed]
        "python" = "python"
        [envs.python-3-11]
        channels = ["conda-forge"]
        [envs.python-3-11.dependencies]
        python = "3.11"
        [envs.python-3-11.exposed]
        "python" = "python"
        "#;
        let manifest = ParsedManifest::from_toml_str(contents);

        assert!(manifest.is_err());
        assert_snapshot!(manifest.unwrap_err());
    }

    #[test]
    fn test_duplicate_dependency() {
        let contents = r#"
        [envs.python]
        channels = ["conda-forge"]
        [envs.python.dependencies]
        python = "*"
        PYTHON = "*"
        [envs.python.exposed]
        python = "python"
        "#;
        let manifest = ParsedManifest::from_toml_str(contents);

        assert!(manifest.is_err());
        assert_snapshot!(manifest.unwrap_err());
    }

    #[test]
    fn test_tool_deserialization() {
        let contents = r#"
        # The name of the environment is `python`
        [envs.python]
        channels = ["conda-forge"]
        # optional, defaults to your current OS
        platform = "osx-64"
        # It will expose python, python3 and python3.11, but not pip
        [envs.python.dependencies]
        python = "3.11.*"
        pip = "*"

        [envs.python.exposed]
        python = "python"
        python3 = "python3"
        "python3.11" = "python3.11"

        # The name of the environment is `python3-10`
        [envs.python3-10]
        channels = ["https://fast.prefix.dev/conda-forge"]
        # It will expose python3.10
        [envs.python3-10.dependencies]
        python = "3.10.*"

        [envs.python3-10.exposed]
        "python3.10" = "python"
        "#;
        let _manifest = ParsedManifest::from_toml_str(contents).unwrap();
    }
}
