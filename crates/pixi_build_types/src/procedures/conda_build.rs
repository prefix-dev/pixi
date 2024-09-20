use std::path::PathBuf;

use rattler_conda_types::Platform;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::ChannelConfiguration;

pub const METHOD_NAME: &str = "conda/build";

/// Parameters for the `conda/build` request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CondaBuildParams {
    /// The target platform that the metadata should be fetched for.
    pub target_platform: Option<Platform>,

    /// The channel base URLs that the metadata should be fetched from.
    pub channel_base_urls: Option<Vec<Url>>,

    /// The channel configuration to use to resolve dependencies.
    pub channel_configuration: ChannelConfiguration,

    /// Information about the output to build. This information is previously
    /// returned from a call to `conda/getMetadata`.
    #[serde(default)]
    pub output: CondaOutputIdentifier,
}

/// Identifier of an output.
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct CondaOutputIdentifier {
    pub name: Option<String>,
    pub version: Option<String>,
    pub build: Option<String>,
    pub subdir: Option<String>,
}

/// Contains the result of the `conda/build` request.
#[derive(Debug, Serialize, Deserialize)]
pub struct CondaBuildResult {
    // TODO: Should this be a UTF8 encoded type
    pub output_file: PathBuf,
    /// The globs that were used as input to the build.
    /// use these for re-verifying the build.
    pub input_globs: Vec<String>,
}
