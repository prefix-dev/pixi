use rattler_conda_types::Platform;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{ChannelConfiguration, CondaPackageMetadata};

pub const METHOD_NAME: &str = "conda/getMetadata";

/// Parameters for the `conda/getMetadata` request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CondaMetadataParams {
    /// The target platform that the metadata should be fetched for.
    pub target_platform: Option<Platform>,

    /// The channel base URLs that the metadata should be fetched from.
    pub channel_base_urls: Option<Vec<Url>>,

    /// The channel configuration to use to resolve dependencies.
    pub channel_configuration: ChannelConfiguration,
}

/// Contains the result of the `conda/getMetadata` request.
#[derive(Debug, Serialize, Deserialize)]
pub struct CondaMetadataResult {
    pub packages: Vec<CondaPackageMetadata>,
}
