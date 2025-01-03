use serde::{Deserialize, Serialize};
use serde_with::serde_as;

/// Custom S3 configuration
#[serde_as]
#[derive(Debug, Clone, PartialEq, Serialize, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct S3Options {
    /// S3 endpoint URL
    pub endpoint_url: Option<String>,
    /// Name of the region
    pub region: Option<String>,
    /// Force path style URLs instead of subdomain style
    pub force_path_style: Option<bool>,
}
