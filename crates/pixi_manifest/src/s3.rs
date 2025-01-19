use serde::{Deserialize, Serialize};
use url::Url;

/// Custom S3 configuration
#[derive(Debug, Clone, PartialEq, Serialize, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct S3Options {
    /// S3 endpoint URL
    pub endpoint_url: Url,
    /// Name of the region
    pub region: String,
    /// Force path style URLs instead of subdomain style
    pub force_path_style: bool,
}
