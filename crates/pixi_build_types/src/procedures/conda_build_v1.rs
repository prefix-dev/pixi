//! This API was introduced in Pixi Build API version 1.
//!
//! This is an iteration of the `conda/build` API where the client is expected
//! to set up the build environment. This allows the client to orchestrate
//! source dependencies and other build steps before the backend is invoked to
//! build the package.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use rattler_conda_types::{ChannelUrl, PackageName, Platform, RepoDataRecord, VersionWithSource};
use serde::{Deserialize, Serialize};

pub const METHOD_NAME: &str = "conda/build_v1";

/// Parameters for the `conda/build_v1` request.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CondaBuildV1Params {
    /// The canonical channel URLs that define where dependencies will be
    /// fetched from. Although this information is not immediately useful for
    /// the backend, the backend may choose to generate a different recipe based
    /// on the channels.
    #[serde(default)]
    pub channels: Vec<ChannelUrl>,

    /// The path to the build prefix, or `None` if no build prefix is created.
    pub build_prefix: Option<CondaBuildV1Prefix>,

    /// The path to the host prefix, or `None` if no host prefix is created.
    pub host_prefix: Option<CondaBuildV1Prefix>,

    /// The output to build.
    pub output: CondaBuildV1Output,

    /// A directory that can be used by the backend to store files for
    /// subsequent requests. This directory is unique for each source
    /// dependency. This allows backends to perform incremental builds.
    ///
    /// The directory may not yet exist.
    pub work_directory: PathBuf,

    /// The location where to place the built package. If this is `None` the
    /// build backend is free to place the package anywhere.
    pub output_directory: Option<PathBuf>,

    /// Whether we want to install the package as editable
    // TODO: remove this parameter as soon as we have profiles
    pub editable: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CondaBuildV1Prefix {
    /// The path to the prefix.
    pub prefix: PathBuf,

    /// The platform for which the packages were installed.
    pub platform: Platform,

    /// The packages that are installed in the prefix.
    #[serde(default)]
    pub packages: Vec<CondaBuildV1PrefixPackage>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CondaBuildV1PrefixPackage {
    /// The repodata record of the package that was installed in the prefix.
    #[serde(flatten)]
    pub repodata_record: RepoDataRecord,
    // TODO: Add information about how the package was introduced into the prefix. E.g. it was
    //  directly requested in the spec, or as a run_export, etc.
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CondaBuildV1Output {
    /// The name of the package
    pub name: PackageName,

    /// The version of the package.
    ///
    /// This may be `None` if the version is dynamic and thus not statically
    /// known. The backend should take a "best guess" if there are multiple
    /// outputs with different versions.
    pub version: Option<VersionWithSource>,

    /// The build string of the package.
    ///
    /// This may be `None` if the build string is dynamic and thus not
    /// statically known. The backend should take a "best guess" if there
    /// are multiple outputs with different build strings.
    pub build: Option<String>,

    /// The subdirectory of the package, e.g. `linux-64`, `osx-64`, etc.
    pub subdir: Platform,

    /// The variant configuration for the package.
    pub variant: BTreeMap<String, String>,
}

/// Contains the result of the `conda/build_v1` request.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CondaBuildV1Result {
    /// The location on disk where the built package is located.
    ///
    /// If the `output_directory` parameter was provided in the input, the
    /// package should reside in that directory.
    pub output_file: PathBuf,

    /// The globs that were used as input to the build. If any of the files that
    /// match these globs changes, the package should be considered
    /// "out-of-date".
    pub input_globs: BTreeSet<String>,

    /// The normalized name of the package.
    pub name: String,

    /// The version of the package.
    pub version: VersionWithSource,

    /// The build string of the package.
    pub build: String,

    /// The subdirectory of the package.
    pub subdir: Platform,
}
