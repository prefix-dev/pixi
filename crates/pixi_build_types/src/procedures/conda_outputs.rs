//! Describes the `conda/outputs` request and its parameters.
//!
//! This request is used to compute all the outputs that a particular backend
//! can provide. It returns the identifiable metadata of the outputs, including
//! the dependencies required to be able to build them.
//!
//! This API was introduced in Pixi Build API version 1.

use ordermap::OrderSet;
use rattler_conda_types::{NoArchType, PackageName, Platform, VersionWithSource};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
};

use crate::{BinaryPackageSpecV1, PackageSpecV1, project_model::NamedSpecV1};

pub const METHOD_NAME: &str = "conda/outputs";

/// Parameters for the `conda/outputs` request.
///
/// The result of this request should be a list of packages that can be built by
/// this particular backend.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CondaOutputsParams {
    /// The native platform for which the outputs should be computed.
    ///
    /// This is usually the same platform as the platform on which the backend
    /// is running but when cross-compiling this could be different.
    pub host_platform: Platform,

    /// The possible variants by the pixi workspace.
    pub variant_configuration: Option<HashMap<String, Vec<String>>>,

    /// A directory that can be used by the backend to store files for
    /// subsequent requests. This directory is unique for each separate source
    /// dependency.
    ///
    /// The directory may not yet exist.
    pub work_directory: PathBuf,
}

/// Contains the result of the `conda/outputs` request.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CondaOutputsResult {
    /// Metadata of all the packages that can be built.
    pub outputs: Vec<CondaOutputMetadata>,

    /// The files that were read as part of the computation. These files are
    /// hashed and stored in the lock-file. If the files change, the
    /// lock-file will be invalidated.
    ///
    /// If this field is not present, the input manifest will be used.
    pub input_globs: Option<BTreeSet<String>>,
}

#[serde_as]
#[derive(Debug, Serialize, Deserialize, Clone, Hash, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CondaCacheMetadata {
    /// An optional name
    pub name: Option<String>,

    /// The build dependencies of the package. These refer to the packages that
    /// should be installed in the "build" environment. The build environment
    /// contains packages for the current architecture that can be used to run
    /// tools on the current machine like compilers, code generators, etc.
    pub build_dependencies: Option<CondaOutputDependencies>,

    /// The "host" dependencies of the package. These refer to the package that
    /// should be installed to be able to refer to them from the build process
    /// but not run them. They are installed for the "target" architecture (see
    /// subdir) or for the current architecture if the target is `noarch`.
    ///
    /// For C++ packages these would be libraries to link against.
    pub host_dependencies: Option<CondaOutputDependencies>,

    /// Describes which run-exports should be ignored for this package.
    pub ignore_run_exports: CondaOutputIgnoreRunExports,
}

#[serde_as]
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CondaOutputMetadata {
    /// The identifier of the output.
    pub identifier: CondaOutputIdentifier,

    /// The build dependencies of the package. These refer to the packages that
    /// should be installed in the "build" environment. The build environment
    /// contains packages for the current architecture that can be used to run
    /// tools on the current machine like compilers, code generators, etc.
    pub build_dependencies: Option<CondaOutputDependencies>,

    /// The "host" dependencies of the package. These refer to the package that
    /// should be installed to be able to refer to them from the build process
    /// but not run them. They are installed for the "target" architecture (see
    /// subdir) or for the current architecture if the target is `noarch`.
    ///
    /// For C++ packages these would be libraries to link against.
    pub host_dependencies: Option<CondaOutputDependencies>,

    /// The dependencies for the run environment of the package. These
    /// dependencies are installed at runtime when this particular package is
    /// also installed.
    pub run_dependencies: CondaOutputDependencies,

    /// Describes which run-exports should be ignored for this package.
    pub ignore_run_exports: CondaOutputIgnoreRunExports,

    /// The run exports of this particular output.
    pub run_exports: CondaOutputRunExports,

    /// A cache that might be shared between multiple outputs based on the
    /// contents of the cache.
    pub cache: Option<CondaCacheMetadata>,

    /// Explicit input globs for this specific output. If this is `None`,
    /// [`CondaOutputsResult::input_globs`] will be used.
    pub input_globs: Option<BTreeSet<String>>,
}

#[serde_as]
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CondaOutputIdentifier {
    /// The name of the package.
    pub name: PackageName,

    /// The version of the package.
    pub version: VersionWithSource,

    /// The build hash of the package.
    pub build: String,

    /// The build number of the package.
    pub build_number: u64,

    /// The subdir or platform
    pub subdir: Platform,

    /// The license of the package
    pub license: Option<String>,

    /// The license family of the package
    pub license_family: Option<String>,

    /// The noarch type of the package
    pub noarch: NoArchType,

    /// Any PURL (Package URL) that is associated with this package.
    pub purls: Option<OrderSet<rattler_conda_types::PackageUrl>>,
}

/// Describes dependencies, constraints and source dependencies for a particular
/// environment.
#[serde_as]
#[derive(Debug, Default, Serialize, Deserialize, Clone, Hash, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CondaOutputDependencies {
    /// A list of matchspecs that describe the dependencies of a particular
    /// environment.
    pub depends: Vec<NamedSpecV1<PackageSpecV1>>,

    /// Additional constraints that apply to the environment in which the
    /// dependencies are solved. Constraints are represented as matchspecs.
    pub constraints: Vec<NamedSpecV1<BinaryPackageSpecV1>>,
}

/// Describes which run-exports should be ignored for a particular output.
#[serde_as]
#[derive(Debug, Default, Serialize, Deserialize, Clone, Hash, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CondaOutputIgnoreRunExports {
    /// Run exports to ignore by name of the package that is exported
    pub by_name: Vec<PackageName>,

    /// Run exports to ignore by the package that applies them
    pub from_package: Vec<PackageName>,
}

#[serde_as]
#[derive(Debug, Default, Deserialize, Serialize, Clone, Hash, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CondaOutputRunExports {
    /// weak run exports apply a dependency from host to run
    pub weak: Vec<NamedSpecV1<PackageSpecV1>>,

    /// strong run exports apply a dependency from build to host and run
    pub strong: Vec<NamedSpecV1<PackageSpecV1>>,

    /// noarch run exports apply a run export only to noarch packages (other run
    /// exports are ignored) for example, python uses this to apply a
    /// dependency on python to all noarch packages, but not to
    /// the python_abi package
    pub noarch: Vec<NamedSpecV1<PackageSpecV1>>,

    /// weak constrains apply a constrain dependency from host to build, or run
    /// to host
    pub weak_constrains: Vec<NamedSpecV1<BinaryPackageSpecV1>>,

    /// strong constrains apply a constrain dependency from build to host and
    /// run
    pub strong_constrains: Vec<NamedSpecV1<BinaryPackageSpecV1>>,
}
