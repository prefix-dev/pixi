use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_with::serde_as;

/// Custom S3 configuration
#[serde_as]
#[derive(Debug, Clone, PartialEq, Serialize, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct S3Options {
    /// Config file location
    pub config_file: Option<PathBuf>,
    /// Name of the profile
    pub profile: Option<String>,
    /// Force path style URLs instead of subdomain style
    pub force_path_style: Option<bool>,
}
