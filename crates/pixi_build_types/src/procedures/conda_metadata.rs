use serde::{Deserialize, Serialize};
use url::Url;

use crate::{ChannelConfiguration, CondaPackageMetadata, PlatformAndVirtualPackages};

pub const METHOD_NAME: &str = "conda/getMetadata";

/// Parameters for the `conda/getMetadata` request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CondaMetadataParams {
    /// The platform that will run the build.
    ///
    /// If this field is not present, the current platform should be used.
    pub build_platform: Option<PlatformAndVirtualPackages>,

    /// The platform where the package will run. In the conda world this might
    /// be different from the "target" platform when targeting noarch.
    ///
    /// If this field is not present, the current platform should be used.
    pub host_platform: Option<PlatformAndVirtualPackages>,

    /// The channel base URLs that the metadata should be fetched from.
    pub channel_base_urls: Option<Vec<Url>>,

    /// The channel configuration to use to resolve dependencies.
    pub channel_configuration: ChannelConfiguration,
}

/// Contains the result of the `conda/getMetadata` request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CondaMetadataResult {
    /// Metadata of all the packages that can be build.
    pub packages: Vec<CondaPackageMetadata>,

    /// The files that were read as part of the computation. These files are
    /// hashed and stored in the lock-file. If the files change, the
    /// lock-file will be invalidated.
    ///
    /// If this field is not present, the input manifest will be used.
    #[serde(default)]
    pub input_globs: Option<Vec<String>>,
}
