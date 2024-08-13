use toml_edit::TomlError;

/// Describes the contents of a parsed global project manifest.
#[derive(Debug, Clone)]
pub struct ParsedManifest {
    /// All the environments defined in the project.
    pub environments: Environments,
}

impl ParsedManifest {
    /// Parses a toml string into a project manifest.
    pub fn from_toml_str(source: &str) -> Result<Self, TomlError> {
        toml_edit::de::from_str(source).map_err(TomlError::from)
    }
}
