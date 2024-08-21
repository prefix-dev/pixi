use indexmap::{IndexMap, IndexSet};
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
    dependencies: IndexMap<PackageName, PixiSpec>,
    exposed: IndexMap<String, String>,
}

impl ParsedEnvironment {
    pub(crate) fn dependencies(&self) -> IndexMap<PackageName, PixiSpec> {
        self.dependencies.clone()
    }

    // If `self.platform` is `None` is not given, the current platform is used
    pub(crate) fn platform(&self) -> Platform {
        if let Some(platform) = self.platform {
            platform
        } else {
            Platform::current()
        }
    }

    /// Returns the channels associated with this collection.
    pub(crate) fn channels(&self) -> IndexSet<NamedChannelOrUrl> {
        // The prioritized channels contain a priority, sort on this priority.
        // Higher priority comes first. [-10, 1, 0 ,2] -> [2, 1, 0, -10]
        self.channels
            .clone()
            .sorted_by(|a, b| {
                let a = a.priority.unwrap_or(0);
                let b = b.priority.unwrap_or(0);
                b.cmp(&a)
            })
            .map(|prioritized_channel| prioritized_channel.channel)
            .collect()
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
    fn test_duplicate_dependency() {
        let contents = r#"
        [envs.python.dependencies]
        python = "*"
        PYTHON = "*"
        [envs.python.exposed]
        python = "python"
        "#;
        let manifest = ParsedManifest::from_toml_str(contents);

        assert!(manifest.is_err());
        assert!(manifest
            .unwrap_err()
            .to_string()
            .contains("duplicate dependency"));
    }

    #[test]
    fn test_tool_deserialization() {
        let contents = r#"
        # The name of the environment is `python`
        # It will expose python, python3 and python3.11, but not pip
        [envs.python.dependencies]
        python = "3.11.*"
        pip = "*"

        [envs.python.exposed]
        python = "python"
        python3 = "python3"
        "python3.11" = "python3.11"

        # The name of the environment is `python3-10`
        # It will expose python3.10
        [envs.python3-10.dependencies]
        python = "3.10.*"

        [envs.python3-10.exposed]
        "python3.10" = "python"
        "#;
        let _manifest = ParsedManifest::from_toml_str(contents).unwrap();
    }
}
