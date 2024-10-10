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

    /// Information about the outputs to build. This information is previously
    /// returned from a call to `conda/getMetadata`. Pass `None` to build all
    /// outputs.
    #[serde(default)]
    pub outputs: Option<Vec<CondaOutputIdentifier>>,
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
    /// The packages that were built.
    pub packages: Vec<CondaBuiltPackage>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CondaBuiltPackage {
    /// The location on disk where the built package is located.
    pub output_file: PathBuf,

    /// The globs that were used as input to the build. Use these for
    /// re-verifying the build.
    pub input_globs: Vec<String>,

    /// The name of the package.
    pub name: String,

    /// The version of the package.
    pub version: String,

    /// The build string of the package.
    pub build: String,

    /// The subdirectory of the package.
    pub subdir: String,
}
