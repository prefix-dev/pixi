use pixi_build_types::procedures::conda_outputs::CondaRunExports;
use pixi_record::{InputHash, PinnedSourceSpec};
use pixi_spec::PixiSpec;
use rattler_conda_types::{MatchSpec, NoArchType, PackageName, Platform, VersionWithSource};

/// Represents a source record where the dependencies have not yet been
/// resolved.
#[derive(Debug, Clone)]
pub struct UnresolvedSourceRecord {
    /// The location of the source record.
    pub source: PinnedSourceSpec,

    /// The hash of the input that was used to build the metadata of the
    /// package. This can be used to verify that the metadata is still valid.
    ///
    /// If this is `None`, the input hash was not computed or is not relevant
    /// for this record. The record can always be considered up to date.
    pub input_hash: Option<InputHash>,

    /// The build dependencies of the package. These refer to the packages that
    /// should be installed in the "build" environment. The build environment
    /// contains packages for the current architecture that can be used to run
    /// tools on the current machine like compilers, code generators, etc.
    pub build_dependencies: Option<UnresolvedDependencies>,

    /// The "host" dependencies of the package. These refer to the package that
    /// should be installed to be able to refer to them from the build process
    /// but not run them. They are installed for the "target" architecture (see
    /// subdir) or for the current architecture if the target is `noarch`.
    ///
    /// For C++ packages these would be libraries to link against.
    pub host_dependencies: Option<UnresolvedDependencies>,

    /// The dependencies for the run environment of the package. These
    /// dependencies are installed at runtime when this particular package is
    /// also installed.
    pub run_dependencies: UnresolvedDependencies,

    /// Describes which run-exports should be ignored for this package.
    pub ignore_run_exports: IgnoreRunExports,

    /// The run exports of this particular output.
    pub run_exports: CondaRunExports,

    /// A cache that might be shared between multiple outputs based on the
    /// contents of the cache.
    pub cache: Option<CacheMetadata>,
}

/// Fields that uniquely identify a source record independant from the
/// dependencies.
#[derive(Debug, Clone)]
pub struct SourceRecordIdentifier {
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
}

/// Describes dependencies, constraints and source dependencies for a particular
/// environment.
#[derive(Debug, Clone)]
pub struct UnresolvedDependencies {
    /// A list of matchspecs that describe the dependencies of a particular
    /// environment.
    pub depends: Vec<PixiSpec>,

    /// Additional constraints that apply to the environment in which the
    /// dependencies are solved. Constraints are represented as matchspecs.
    pub constraints: Vec<MatchSpec>,
}

/// Describes which run-exports should be ignored for a particular output.
#[derive(Debug, Clone)]
pub struct IgnoreRunExports {
    /// Run exports to ignore by name of the package that is exported
    pub by_name: Vec<PackageName>,

    /// Run exports to ignore by the package that applies them
    pub from_package: Vec<PackageName>,
}

#[derive(Debug, Clone)]
pub struct RunExports {
    /// weak run exports apply a dependency from host to run
    pub weak: Vec<MatchSpec>,

    /// strong run exports apply a dependency from build to host and run
    pub strong: Vec<MatchSpec>,

    /// noarch run exports apply a run export only to noarch packages (other run
    /// exports are ignored) for example, python uses this to apply a
    /// dependency on python to all noarch packages, but not to
    /// the python_abi package
    pub noarch: Vec<MatchSpec>,

    /// weak constrains apply a constrain dependency from host to build, or run
    /// to host
    pub weak_constrains: Vec<MatchSpec>,

    /// strong constrains apply a constrain dependency from build to host and
    /// run
    pub strong_constrains: Vec<MatchSpec>,
}

#[derive(Debug, Clone)]
pub struct CacheMetadata {
    /// An optional name
    pub name: Option<String>,

    /// The build dependencies of the package. These refer to the packages that
    /// should be installed in the "build" environment. The build environment
    /// contains packages for the current architecture that can be used to run
    /// tools on the current machine like compilers, code generators, etc.
    pub build_dependencies: Option<UnresolvedDependencies>,

    /// The "host" dependencies of the package. These refer to the package that
    /// should be installed to be able to refer to them from the build process
    /// but not run them. They are installed for the "target" architecture (see
    /// subdir) or for the current architecture if the target is `noarch`.
    ///
    /// For C++ packages these would be libraries to link against.
    pub host_dependencies: Option<UnresolvedDependencies>,

    /// Describes which run-exports should be ignored for this package.
    pub ignore_run_exports: IgnoreRunExports,
}
