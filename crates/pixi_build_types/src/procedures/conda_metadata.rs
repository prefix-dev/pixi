use rattler_conda_types::Platform;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::CondaPackageMetadata;

pub const METHOD_NAME: &str = "conda/getMetadata";

/// Parameters for the condaMetadata request.
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

/// Information about the channel configuration to use to resolve dependencies.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelConfiguration {
    /// The default base URL to use for channels when the channel is not
    /// specified as a full URL.
    pub base_url: Url,
}

/// Contains the result of the condaMetadata request.
#[derive(Debug, Serialize, Deserialize)]
pub struct CondaMetadataResult {
    pub packages: Vec<CondaPackageMetadata>,
}
