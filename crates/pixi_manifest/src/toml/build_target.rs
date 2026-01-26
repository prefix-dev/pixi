use toml_span::{DeserError, Value, de_helpers::TableHelper};

use crate::toml::build_backend::convert_toml_to_serde;
use crate::warning::Deprecation;

#[derive(Debug)]
pub struct TomlPackageBuildTarget {
    pub config: Option<serde_value::Value>,
    pub warnings: Vec<crate::Warning>,
}

impl<'de> toml_span::Deserialize<'de> for TomlPackageBuildTarget {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;
        let mut warnings = Vec::new();

        let config = if let Some((_, mut value)) = th.take("config") {
            Some(convert_toml_to_serde(&mut value)?)
        } else if let Some((key, mut value)) = th.table.remove_entry("configuration") {
            warnings.push(Deprecation::renamed_field("configuration", "config", key.span).into());
            Some(convert_toml_to_serde(&mut value)?)
        } else {
            None
        };

        th.finalize(None)?;
        Ok(TomlPackageBuildTarget { config, warnings })
    }
}
