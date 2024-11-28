use crate::utils::PixiSpanned;
use serde::{Deserialize, Deserializer};

/// Helper struct to deserialize the environment from TOML.
/// The environment description can only hold these values.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct TomlEnvironment {
    #[serde(default)]
    pub features: PixiSpanned<Vec<String>>,
    pub solve_group: Option<String>,
    #[serde(default)]
    pub no_default_feature: bool,
}

#[derive(Debug)]
pub enum TomlEnvironmentList {
    Map(TomlEnvironment),
    Seq(Vec<String>),
}

impl<'de> Deserialize<'de> for TomlEnvironmentList {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        serde_untagged::UntaggedEnumVisitor::new()
            .map(|map| map.deserialize().map(TomlEnvironmentList::Map))
            .seq(|seq| seq.deserialize().map(TomlEnvironmentList::Seq))
            .expecting("either a map or a sequence")
            .deserialize(deserializer)
    }
}
