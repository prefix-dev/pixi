use indexmap::IndexMap;
use pixi_manifest::deserialize_package_map;
use rattler_conda_types::PackageName;
use serde_with::{serde_as, serde_derive::Deserialize};

use super::environment::EnvironmentName;

use super::errors::ManifestError;
use pixi_spec::PixiSpec;

/// Describes the contents of a parsed global project manifest.
#[serde_as]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ParsedManifest {
    /// The environments the project can create.
    #[serde(default)]
    envs: IndexMap<EnvironmentName, ParsedEnvironment>,
}

impl ParsedManifest {
    /// Parses a toml string into a project manifest.
    pub fn from_toml_str(source: &str) -> Result<Self, ManifestError> {
        toml_edit::de::from_str(source).map_err(ManifestError::from)
    }
}

#[serde_as]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ParsedEnvironment {
    #[serde(default, deserialize_with = "deserialize_package_map")]
    dependencies: IndexMap<PackageName, PixiSpec>,
    exposed: IndexMap<String, String>,
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;

    use super::ParsedManifest;

    #[test]
    fn test_invalid_key() {
        let examples = ["[invalid]", "[envs.ipython.invalid]"];
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

        # The name of the environment is `python_3_10`
        # It will expose python3.10
        [envs.python_3_10.dependencies]
        python = "3.10.*"

        [envs.python_3_10.exposed]
        "python3.10" = "python"
        "#;
        let _manifest = ParsedManifest::from_toml_str(contents).unwrap();
    }
}
