use indexmap::IndexMap;
use rattler_conda_types::PackageName;
use serde_with::{serde_as, serde_derive::Deserialize};

use super::environment::EnvironmentName;

use pixi_spec::PixiSpec;
use toml_edit::TomlError;

/// Describes the contents of a parsed global project manifest.
#[serde_as]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ParsedManifest {
    /// The environments the project can create.
    envs: IndexMap<EnvironmentName, ParsedEnvironment>,
}

impl ParsedManifest {
    /// Parses a toml string into a project manifest.
    pub fn from_toml_str(source: &str) -> Result<Self, TomlError> {
        toml_edit::de::from_str(source).map_err(TomlError::from)
    }
}

#[serde_as]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ParsedEnvironment {
    dependencies: IndexMap<PackageName, PixiSpec>,
    exposed: IndexMap<String, String>,
}
