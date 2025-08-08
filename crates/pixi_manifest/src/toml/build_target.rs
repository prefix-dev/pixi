use pixi_toml::convert_toml_to_serde;
use toml_span::{DeserError, Value, de_helpers::TableHelper};

#[derive(Debug)]
pub struct TomlPackageBuildTarget {
    pub configuration: Option<serde_value::Value>,
}

impl<'de> toml_span::Deserialize<'de> for TomlPackageBuildTarget {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;
        let configuration = th
            .take("configuration")
            .map(|(_, mut value)| convert_toml_to_serde(&mut value))
            .transpose()?;

        th.finalize(None)?;
        Ok(TomlPackageBuildTarget { configuration })
    }
}
