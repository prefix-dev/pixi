use serde::Deserialize;
use std::collections::HashMap;

#[derive(Default, Clone, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Activation {
    pub scripts: Option<Vec<String>>,
    /// Environment variables to set before running the scripts.
    pub env: Option<HashMap<String, String>>,
}
