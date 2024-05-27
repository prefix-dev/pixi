use indexmap::IndexMap;
use serde::Deserialize;

#[derive(Default, Clone, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Activation {
    pub scripts: Option<Vec<String>>,
    /// Environment variables to set before running the scripts.
    pub env: Option<IndexMap<String, String>>,
}
