use std::{collections::HashMap, path::PathBuf};

use rattler_conda_types::GenericVirtualPackage;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{ChannelConfiguration, PlatformAndVirtualPackages};

pub const METHOD_NAME: &str = "conda/build";

/// Parameters for the `conda/build` request.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CondaBuildParams {
    /// The build platform is always the current platform, but the virtual
    /// packages used can be override.
    ///
    /// If this is not present, the virtual packages from the current platform
    /// are used.
    pub build_platform_virtual_packages: Option<Vec<GenericVirtualPackage>>,

    /// The target platform that the metadata should be fetched for.
    pub host_platform: Option<PlatformAndVirtualPackages>,

    /// The channel base URLs for the conda channels to use to resolve
    pub channel_base_urls: Option<Vec<Url>>,

    /// The channel configuration to use to resolve dependencies.
    pub channel_configuration: ChannelConfiguration,

    /// Information about the outputs to build. This information is previously
    /// returned from a call to `conda/getMetadata`. Pass `None` to build all
    /// outputs.
    #[serde(default)]
    pub outputs: Option<Vec<CondaOutputIdentifier>>,

    /// The variants that we want to build
    pub variant_configuration: Option<HashMap<String, Vec<String>>>,

    /// A directory that can be used by the backend to store files for
    /// subsequent requests. This directory is unique for each separate source
    /// dependency.
    ///
    /// The directory may not yet exist.
    pub work_directory: PathBuf,

    /// Whether we want to install the package as editable
    // TODO: remove this parameter as soon as we have profiles
    pub editable: bool,
}

/// Identifier of an output.
#[derive(Default, Debug, Serialize, Deserialize, Clone)]
pub struct CondaOutputIdentifier {
    pub name: Option<String>,
    pub version: Option<String>,
    pub build: Option<String>,
    pub subdir: Option<String>,
}

/// Contains the result of the `conda/build` request.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CondaBuildResult {
    /// The packages that were built.
    pub packages: Vec<CondaBuiltPackage>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
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
