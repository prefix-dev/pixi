use indexmap::IndexMap;
use serde::Deserialize;
use toml_span::{DeserError, Value};
use pixi_toml::TomlEnum;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Deserialize, strum::Display, strum::EnumString)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum WarningAction {
    Hide,
    #[default]
    Log,
    Verbose,
    Fail,
}

impl<'de> toml_span::Deserialize<'de> for WarningAction {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        TomlEnum::deserialize(value).map(TomlEnum::into_inner)
    }
}

#[derive(Debug, Clone, Default)]
pub struct WarningConfig {
    pub patterns: IndexMap<String, WarningBehavior>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum WarningBehavior {
    Action(WarningAction),
    FatAction {
        pattern: Option<String>,
        level: WarningAction,
        description: Option<String>,
    }
}

impl WarningConfig {
    /// Returns the action for a given warning code.
    pub fn apply_config(&self, code: &crate::warning::WarningCode) -> WarningAction {
         let code_str = code.as_str();
         let short_code = code.short_code();

         for (pattern, behavior) in &self.patterns {
             // Simple regex matching for MVP
             let regex_pattern = pattern.replace("*", ".*");
             if let Ok(re) = regex::Regex::new(&format!("^{}$", regex_pattern)) {
                 if re.is_match(code_str) || re.is_match(short_code) {
                     return match behavior {
                         WarningBehavior::Action(action) => *action,
                         WarningBehavior::FatAction { level, .. } => *level,
                     };
                 }
             }
         }
         WarningAction::Log
    }
}

impl<'de> toml_span::Deserialize<'de> for WarningConfig {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let patterns = IndexMap::<String, WarningBehavior>::deserialize(value)?;
        Ok(WarningConfig { patterns })
    }
}

impl<'de> toml_span::Deserialize<'de> for WarningBehavior {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        if value.as_str().is_some() {
             let action = WarningAction::deserialize(value)?;
             Ok(WarningBehavior::Action(action))
        } else {
             // Handle fat action
             let mut th = toml_span::de_helpers::TableHelper::new(value)?;
             let pattern = th.optional("pattern");
             let level = th.optional("level").unwrap_or_default();
             let description = th.optional("description");
             th.finalize(None)?;
             Ok(WarningBehavior::FatAction { pattern, level, description })
        }
    }
}
