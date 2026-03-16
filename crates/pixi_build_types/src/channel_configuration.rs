use serde::{Deserialize, Serialize};
use url::Url;

/// Information about the channel configuration to use to resolve dependencies.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChannelConfiguration {
    /// The default base URL to use for channels when the channel is not
    /// specified as a full URL.
    pub base_url: Url,
}
