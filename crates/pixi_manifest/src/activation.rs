use indexmap::IndexMap;
use toml_span::{DeserError, Value, de_helpers::TableHelper};

use pixi_toml::TomlIndexMap;

#[derive(Default, Clone, Debug)]
pub struct Activation {
    pub scripts: Option<Vec<String>>,
    /// Environment variables to set before running the scripts.
    pub env: Option<IndexMap<String, String>>,
}

impl<'de> toml_span::Deserialize<'de> for Activation {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;
        let scripts = th.optional("scripts");
        let env = th.optional::<TomlIndexMap<_, _>>("env");
        th.finalize(None)?;
        Ok(Activation {
            scripts,
            env: env.map(TomlIndexMap::into_inner),
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::toml::FromTomlStr;

    #[test]
    fn deserialize_activation() {
        let input = r#"
            scripts = ["echo 'Hello, World!'"]
            [env]
            FOO = "bar"
            "#;

        let activation = Activation::from_toml_str(input).unwrap();
        assert_eq!(
            activation.scripts,
            Some(vec!["echo 'Hello, World!'".to_string()])
        );
        assert_eq!(
            activation.env,
            Some(IndexMap::from_iter(vec![(
                "FOO".to_string(),
                "bar".to_string()
            )]))
        );
    }
}
