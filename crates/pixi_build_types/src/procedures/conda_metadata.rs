use rattler_conda_types::Platform;
use serde::{Deserialize, Serialize};

use crate::{CondaPackageMetadata, CondaUrl};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Parameters for the condaMetadata request.
pub struct CondaMetadataParams {
    /// The target platform that the metadata should be fetched for.
    pub target_platform: Option<Platform>,

    /// The channel base URLs that the metadata should be fetched from.
    pub channel_base_urls: Option<Vec<CondaUrl>>,
}

#[derive(Debug, Serialize, Deserialize)]
/// Contains the result of the condaMetadata request.
pub struct CondaMetadataResult {
    pub packages: Vec<CondaPackageMetadata>,
}
