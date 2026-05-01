use std::{
    collections::HashSet,
    fmt::{Display, Formatter},
    path::PathBuf,
};

use itertools::Itertools;
use miette::Diagnostic;
use pep440_rs::VersionSpecifiers;
use pixi_command_dispatcher::{DevSourceMetadataError, SourceCheckoutError, SourceRecordError};
use pixi_manifest::pypi::pypi_options::PrereleaseMode;
use pixi_record::{ParseLockFileError, SourceMismatchError};
use pixi_uv_conversions::AsPep508Error;
use rattler_conda_types::{
    MatchSpec, PackageName, ParseChannelError, ParseMatchSpecError, Platform,
};
use rattler_lock::{PackageHashes, PypiIndexes};
use thiserror::Error;
use url::Url;
use uv_distribution_filename::ExtensionError;
use uv_pypi_types::ParsedUrlError;

use super::pypi_metadata;
use crate::{lock_file::package_identifier::ConversionError, workspace::errors::VariantsError};

#[derive(Debug, Error, Diagnostic)]
pub enum EnvironmentUnsat {
    #[error("the channels in the lock-file do not match the environments channels")]
    ChannelsMismatch,

    #[error("channels were extended with additional lower-priority channels")]
    ChannelsExtended,

    #[error("platform(s) '{platforms}' present in the lock-file but not in the environment", platforms = .0.iter().map(|p| p.as_str()).join(", ")
    )]
    AdditionalPlatformsInLockFile(HashSet<Platform>),

    #[error(transparent)]
    IndexesMismatch(#[from] IndexesMismatch),

    #[error(transparent)]
    InvalidChannel(#[from] ParseChannelError),

    #[error(transparent)]
    InvalidDistExtensionInNoBuild(#[from] ExtensionError),

    #[error(
        "the lock-file contains non-binary package: '{0}', but the pypi-option `no-build` is set"
    )]
    NoBuildWithNonBinaryPackages(String),

    #[error(
        "the lock-file was solved with a different strategy ({locked_strategy}) than the one selected ({expected_strategy})",
        locked_strategy = fmt_solve_strategy(*.locked_strategy),
        expected_strategy = fmt_solve_strategy(*.expected_strategy),
    )]
    SolveStrategyMismatch {
        locked_strategy: rattler_solve::SolveStrategy,
        expected_strategy: rattler_solve::SolveStrategy,
    },

    #[error(
        "the lock-file was solved with a different channel priority ({locked_priority}) than the one selected ({expected_priority})",
        locked_priority = fmt_channel_priority(*.locked_priority),
        expected_priority = fmt_channel_priority(*.expected_priority),
    )]
    ChannelPriorityMismatch {
        locked_priority: rattler_solve::ChannelPriority,
        expected_priority: rattler_solve::ChannelPriority,
    },

    #[error(
        "the lock-file was solved with a different PyPI prerelease mode ({locked_mode}) than the one selected ({expected_mode})"
    )]
    PypiPrereleaseModeMismatch {
        locked_mode: PrereleaseMode,
        expected_mode: PrereleaseMode,
    },
    #[error(
        "the lock-file was solved with system requirements incompatible with the tags on wheel ({wheel})"
    )]
    PypiWheelTagsMismatch { wheel: String },

    #[error(transparent)]
    ExcludeNewerMismatch(#[from] ExcludeNewerMismatch),

    #[error(transparent)]
    SourceExcludeNewerMismatch(#[from] SourceExcludeNewerMismatch),
}

fn fmt_channel_priority(priority: rattler_solve::ChannelPriority) -> &'static str {
    match priority {
        rattler_solve::ChannelPriority::Strict => "strict",
        rattler_solve::ChannelPriority::Disabled => "disabled",
    }
}

fn fmt_solve_strategy(strategy: rattler_solve::SolveStrategy) -> &'static str {
    match strategy {
        rattler_solve::SolveStrategy::Highest => "highest",
        rattler_solve::SolveStrategy::LowestVersion => "lowest-version",
        rattler_solve::SolveStrategy::LowestVersionDirect => "lowest-version-direct",
    }
}

#[derive(Debug, Error)]
pub struct ExcludeNewerMismatch {
    package: String,
    timestamp: chrono::DateTime<chrono::Utc>,
    exclude_newer: chrono::DateTime<chrono::Utc>,
}

impl Display for ExcludeNewerMismatch {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the locked package '{}' has timestamp {}, which is newer than the environment's exclude-newer cutoff {}",
            self.package, self.timestamp, self.exclude_newer
        )
    }
}

pub(super) fn verify_exclude_newer(
    exclude_newer: Option<&rattler_solve::ExcludeNewer>,
    locked_environment: &rattler_lock::Environment<'_>,
) -> Result<(), ExcludeNewerMismatch> {
    let Some(exclude_newer) = exclude_newer else {
        return Ok(());
    };

    for (_platform, packages) in locked_environment.conda_packages_by_platform() {
        for package in packages {
            let Some(record) = package.record() else {
                continue;
            };
            let channel = package
                .as_binary()
                .and_then(|binary| binary.channel.as_ref())
                .map(ToString::to_string);

            if let Some(timestamp) = record.timestamp.as_ref()
                && exclude_newer.is_excluded(&record.name, channel.as_deref(), Some(timestamp))
            {
                return Err(ExcludeNewerMismatch {
                    package: record.name.as_source().to_string(),
                    timestamp: (*timestamp).into(),
                    exclude_newer: exclude_newer
                        .cutoff_for_package(&record.name, channel.as_deref()),
                });
            }
        }
    }

    Ok(())
}

#[derive(Debug, Error)]
#[error(
    "the locked source package '{package}' has timestamps that exceed the environment's exclude-newer cutoff"
)]
pub struct SourceExcludeNewerMismatch {
    package: String,
}

#[derive(Debug, Error)]
pub struct IndexesMismatch {
    pub(super) current: PypiIndexes,
    pub(super) previous: Option<PypiIndexes>,
}

impl Display for IndexesMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(previous) = &self.previous {
            write!(
                f,
                "the indexes used to previously solve to lock file do not match the environments indexes.\n \
                Expected: {expected:#?}\n Found: {found:#?}",
                expected = previous,
                found = self.current
            )
        } else {
            write!(
                f,
                "the indexes used to previously solve to lock file are missing"
            )
        }
    }
}

#[derive(Debug, Error)]
pub struct SourceTreeHashMismatch {
    pub computed: PackageHashes,
    pub locked: Option<PackageHashes>,
}

impl Display for SourceTreeHashMismatch {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let computed_hash = self
            .computed
            .sha256()
            .map(|hash| format!("{hash:x}"))
            .or(self.computed.md5().map(|hash| format!("{hash:x}")));
        let locked_hash = self.locked.as_ref().and_then(|hash| {
            hash.sha256()
                .map(|hash| format!("{hash:x}"))
                .or(hash.md5().map(|hash| format!("{hash:x}")))
        });

        match (computed_hash, locked_hash) {
            (None, None) => write!(f, "could not compute a source tree hash"),
            (Some(computed), None) => {
                write!(
                    f,
                    "the computed source tree hash is '{computed}', but the lock-file does not contain a hash"
                )
            }
            (Some(computed), Some(locked)) => write!(
                f,
                "the computed source tree hash is '{computed}', but the lock-file contains '{locked}'"
            ),
            (None, Some(locked)) => write!(
                f,
                "could not compute a source tree hash, but the lock-file contains '{locked}'"
            ),
        }
    }
}

/// Describes what metadata changed for a local package.
#[derive(Debug, Error)]
pub enum LocalMetadataMismatch {
    #[error("dependencies changed - added: [{added}], removed: [{removed}]",
        added = format_requirements(added),
        removed = format_requirements(removed))]
    RequiresDist {
        added: Vec<pep508_rs::Requirement>,
        removed: Vec<pep508_rs::Requirement>,
    },
    #[error("version changed from {locked} to {current}")]
    Version {
        locked: pep440_rs::Version,
        current: pep440_rs::Version,
    },
    #[error("requires-python changed from {locked:?} to {current:?}")]
    RequiresPython {
        locked: Option<VersionSpecifiers>,
        current: Option<VersionSpecifiers>,
    },
}

/// Formats a list of requirements, showing only the first 3 names.
fn format_requirements(reqs: &[pep508_rs::Requirement]) -> String {
    const MAX_DISPLAY: usize = 3;
    let names: Vec<_> = reqs
        .iter()
        .take(MAX_DISPLAY)
        .map(|r| r.name.to_string())
        .collect();
    let formatted = names.join(", ");
    if reqs.len() > MAX_DISPLAY {
        format!("{}, ... and {} more", formatted, reqs.len() - MAX_DISPLAY)
    } else {
        formatted
    }
}

impl From<pypi_metadata::MetadataMismatch> for LocalMetadataMismatch {
    fn from(mismatch: pypi_metadata::MetadataMismatch) -> Self {
        match mismatch {
            pypi_metadata::MetadataMismatch::RequiresDist(diff) => {
                LocalMetadataMismatch::RequiresDist {
                    added: diff.added,
                    removed: diff.removed,
                }
            }
            pypi_metadata::MetadataMismatch::Version { locked, current } => {
                LocalMetadataMismatch::Version { locked, current }
            }
            pypi_metadata::MetadataMismatch::RequiresPython { locked, current } => {
                LocalMetadataMismatch::RequiresPython { locked, current }
            }
        }
    }
}

/// Which sub-environment of a source build a satisfiability error
/// applies to. Used by `PlatformUnsat` variants that report mismatches
/// between a backend's declared build/host specs and a source record's
/// locked build/host packages.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum BuildOrHostEnv {
    Build,
    Host,
}

impl Display for BuildOrHostEnv {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildOrHostEnv::Build => write!(f, "build"),
            BuildOrHostEnv::Host => write!(f, "host"),
        }
    }
}

/// Whether a [`PlatformUnsat::SourceRunDependenciesChanged`] mismatch
/// concerns the run-`depends` or the run-`constrains` of a built source
/// package.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SourceRunDepKind {
    /// Mismatch in the run-time `depends` list.
    RunDepends,
    /// Mismatch in the run-time `constrains` list.
    RunConstrains,
}

impl Display for SourceRunDepKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceRunDepKind::RunDepends => write!(f, "run-dependencies"),
            SourceRunDepKind::RunConstrains => write!(f, "run-constraints"),
        }
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum PlatformUnsat {
    #[error("the requirement '{0}' could not be satisfied (required by '{1}')")]
    UnsatisfiableMatchSpec(Box<MatchSpec>, String),

    #[error("no package named '{0}' exists (required by '{1}')")]
    SourcePackageMissing(String, String),

    #[error("required source package '{0}' is locked as binary (required by '{1}')")]
    RequiredSourceIsBinary(String, String),

    #[error("package '{0}' is locked as source, but is only required as binary")]
    RequiredBinaryIsSource(String),

    #[error("the locked source package '{0}' does not match the requested source package, {1}")]
    SourcePackageMismatch(String, SourceMismatchError),

    #[error("failed to convert the requirement for '{0}'")]
    FailedToConvertRequirement(pep508_rs::PackageName, #[source] Box<ParsedUrlError>),

    #[error("the requirement '{0}' could not be satisfied (required by '{1}')")]
    UnsatisfiableRequirement(Box<uv_distribution_types::Requirement>, String),

    #[error("the conda package does not satisfy the pypi requirement '{0}' (required by '{1}')")]
    CondaUnsatisfiableRequirement(Box<uv_distribution_types::Requirement>, String),

    #[error("there was a duplicate entry for '{0}'")]
    DuplicateEntry(String),

    #[error(
        "failed to recompute the build/host environments of a source record loaded from a pre-v7 lock file: {0}"
    )]
    LegacySourceEnvReify(String),

    #[error("the requirement '{0}' failed to parse")]
    FailedToParseMatchSpec(String, #[source] ParseMatchSpecError),

    #[error("there are more conda packages in the lock-file than are used by the environment: {}", .0.iter().map(rattler_conda_types::PackageName::as_source).format(", ")
    )]
    TooManyCondaPackages(Vec<PackageName>),

    #[error("missing purls")]
    MissingPurls,

    #[error("corrupted lock-file entry for '{0}'")]
    CorruptedEntry(String, ParseLockFileError),

    #[error("there are more pypi packages in the lock-file than are used by the environment: {}", .0.iter().format(", ")
    )]
    TooManyPypiPackages(Vec<pep508_rs::PackageName>),

    #[error("there are PyPi dependencies but a python interpreter is missing from the lock-file")]
    MissingPythonInterpreter,

    #[error(
        "a marker environment could not be derived from the python interpreter in the lock-file"
    )]
    FailedToDetermineMarkerEnvironment(#[source] Box<dyn Diagnostic + Send + Sync>),

    #[error(
        "'{0}' requires python version {1} but the python interpreter in the lock-file has version {2}"
    )]
    PythonVersionMismatch(
        pep508_rs::PackageName,
        VersionSpecifiers,
        Box<pep440_rs::Version>,
    ),

    #[error("when converting {0} into a pep508 requirement")]
    AsPep508Error(pep508_rs::PackageName, #[source] AsPep508Error),

    #[error("editable pypi dependency on conda resolved package '{0}' is not supported")]
    EditableDependencyOnCondaInstalledPackage(
        uv_normalize::PackageName,
        Box<uv_distribution_types::RequirementSource>,
    ),

    #[error("direct pypi url dependency to a conda installed package '{0}' is not supported")]
    DirectUrlDependencyOnCondaInstalledPackage(uv_normalize::PackageName),

    #[error("git dependency on a conda installed package '{0}' is not supported")]
    GitDependencyOnCondaInstalledPackage(uv_normalize::PackageName),

    #[error("path dependency on a conda installed package '{0}' is not supported")]
    PathDependencyOnCondaInstalledPackage(uv_normalize::PackageName),

    #[error("directory dependency on a conda installed package '{0}' is not supported")]
    DirectoryDependencyOnCondaInstalledPackage(uv_normalize::PackageName),

    #[error(
        "the editable package '{0}' was expected to be a directory but is a url, which cannot be editable: '{1}'"
    )]
    EditablePackageIsUrl(uv_normalize::PackageName, String),

    #[error("the editable package path '{0}', lock does not equal spec path '{1}' == '{2}'")]
    EditablePackagePathMismatch(uv_normalize::PackageName, PathBuf, PathBuf),

    #[error("failed to determine pypi source tree hash for {0}")]
    FailedToDetermineSourceTreeHash(pep508_rs::PackageName, std::io::Error),

    #[error("source tree hash for {0} does not match the hash in the lock-file")]
    SourceTreeHashMismatch(pep508_rs::PackageName, #[source] SourceTreeHashMismatch),

    #[error("metadata for local package '{0}' has changed: {1}")]
    LocalPackageMetadataMismatch(pep508_rs::PackageName, LocalMetadataMismatch),

    #[error("failed to read metadata for local package '{0}': {1}")]
    FailedToReadLocalMetadata(pep508_rs::PackageName, String),

    #[error("local package '{0}' has dynamic {1} metadata that requires re-resolution")]
    LocalPackageHasDynamicMetadata(pep508_rs::PackageName, &'static str),

    #[error("the path '{0}, cannot be canonicalized")]
    FailedToCanonicalizePath(PathBuf, #[source] std::io::Error),

    #[error("expect pypi package name '{expected}' but found '{found}'")]
    LockedPyPINamesMismatch { expected: String, found: String },

    #[error(
        "'{name}' with specifiers '{specifiers}' does not match the locked version '{version}' "
    )]
    LockedPyPIVersionsMismatch {
        name: String,
        specifiers: String,
        version: String,
    },

    #[error("the direct url should start with `direct+` or `git+` but found '{0}'")]
    LockedPyPIMalformedUrl(Url),

    #[error("the spec for '{0}' required a direct url but it was not locked as such")]
    LockedPyPIRequiresDirectUrl(String),

    #[error("'{name}' has mismatching url: '{spec_url} != {lock_url}'")]
    LockedPyPIDirectUrlMismatch {
        name: String,
        spec_url: String,
        lock_url: String,
    },

    #[error("'{name}' has mismatching git url: '{spec_url} != {lock_url}'")]
    LockedPyPIGitUrlMismatch {
        name: String,
        spec_url: String,
        lock_url: String,
    },

    #[error(
        "'{name}' has mismatching git subdirectory: '{spec_subdirectory} != {lock_subdirectory}'"
    )]
    LockedPyPIGitSubdirectoryMismatch {
        name: String,
        spec_subdirectory: String,
        lock_subdirectory: String,
    },

    #[error("'{name}' has mismatching git ref: '{expected_ref} != {found_ref}'")]
    LockedPyPIGitRefMismatch {
        name: String,
        expected_ref: String,
        found_ref: String,
    },

    #[error("'{0}' expected a git url but the lock file has: '{1}'")]
    LockedPyPIRequiresGitUrl(String, String),

    #[error("'{0}' expected a path but the lock file has a url")]
    LockedPyPIRequiresPath(String),

    #[error(
        "'{name}' absolute required path is {install_path} but currently locked at {locked_path}"
    )]
    LockedPyPIPathMismatch {
        name: String,
        install_path: String,
        locked_path: String,
    },

    #[error("'{name}' requires index {expected_index} but the lock-file has {locked_index}")]
    LockedPyPIIndexMismatch {
        name: String,
        expected_index: String,
        locked_index: String,
    },

    #[error("failed to convert between pep508 and uv types: {0}")]
    UvTypesConversionError(#[from] ConversionError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    BackendDiscovery(#[from] pixi_build_discovery::DiscoveryError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Variants(#[from] VariantsError),

    #[error("'{name}' is locked as a conda package but only requested by pypi dependencies")]
    CondaPackageShouldBePypi { name: String },

    #[error("'{name}' is locked as a distribution but points to a local source directory")]
    DistributionShouldBeSource { name: pep508_rs::PackageName },

    #[error(transparent)]
    InvalidChannel(#[from] ParseChannelError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    SourceCheckout(#[from] SourceCheckoutError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    DevSourceMetadata(#[from] DevSourceMetadataError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    SourceRecord(SourceRecordError),

    #[error("source package '{package}' requires rebuild or re-evaluation; forcing a full re-lock")]
    SourceRecordRequiresRebuild { package: String },

    #[error(
        "no output of source package '{package}' from '{manifest_source}' matches the locked variants ({variants})"
    )]
    SourceVariantNotInBackend {
        package: String,
        manifest_source: String,
        variants: String,
    },

    #[error(
        "the {env}-environment of source package '{package}' no longer satisfies the backend-declared dependency '{spec}'; locked packages: {locked}"
    )]
    SourceBuildHostUnsat {
        package: String,
        env: BuildOrHostEnv,
        spec: String,
        locked: String,
    },

    #[error(
        "the {env}-environment of source package '{package}' contains a pypi-style dependency on '{name}' which is not allowed in build/host environments"
    )]
    SourceBuildHostDisallowsPypi {
        package: String,
        env: BuildOrHostEnv,
        name: String,
    },

    #[error(
        "the {env}-environment of source package '{package}' is missing a record satisfying the backend-declared source dependency '{name}' (requested from '{location}')"
    )]
    SourceBuildHostSourceMissing {
        package: String,
        env: BuildOrHostEnv,
        name: String,
        location: String,
    },

    #[error(
        "the resolved {kind} of source package '{package}' no longer match what the backend would re-derive from the manifest{added_msg}{removed_msg}",
        added_msg = if added.is_empty() { String::new() } else { format!("; added: {}", added.join(", ")) },
        removed_msg = if removed.is_empty() { String::new() } else { format!("; removed: {}", removed.join(", ")) },
    )]
    SourceRunDependenciesChanged {
        /// The source package whose `depends`/`constrains` drifted.
        package: String,
        /// Which side drifted: run-`depends` or run-`constrains`.
        kind: SourceRunDepKind,
        /// Specs the backend now declares that the locked record is missing.
        added: Vec<String>,
        /// Specs the locked record carries that the backend no longer declares.
        removed: Vec<String>,
    },

    #[error(
        "the locked package build source for '{0}' does not match the requested build source, {1}"
    )]
    PackageBuildSourceMismatch(String, SourceMismatchError),

    #[error("the metadata of source package '{0}' changed: {1}")]
    SourcePackageMetadataChanged(String, String),

    #[error("the source location '{0}' changed from '{1}' to '{2}'")]
    SourceBuildLocationChanged(String, String, String),

    #[error(
        "the source dependency '{dependency}' of package '{package}' changed from '{locked}' to '{current}'"
    )]
    SourceDependencyChanged {
        package: String,
        dependency: String,
        locked: String,
        current: String,
    },

    #[error(
        "locked source package '{package_name}' not found in current metadata for '{manifest_path}'. Was the package renamed?"
    )]
    SourcePackageNotFoundInMetadata {
        package_name: String,
        manifest_path: String,
    },

    #[error(
        "locked source package '{package}' does not match any of the outputs in the metadata of the package at '{manifest_path}', only the following outputs are available: {available}"
    )]
    NoMatchingSourcePackageInMetadata {
        package: String,
        manifest_path: String,
        available: String,
    },

    #[error(
        "the locked package '{package}' with version '{locked_version}' does not satisfy the constraint '{constraint}'"
    )]
    ConstraintViolated {
        package: String,
        locked_version: String,
        constraint: String,
    },

    #[error(
        "source specifications are not supported in the `[constraints]` table, but a source constraint was found for '{0}'"
    )]
    SourceConstraintNotSupported(String),

    #[error("failed to build metadata for local package '{0}': {1}")]
    FailedToBuildLocalMetadata(pep508_rs::PackageName, String),
}

#[derive(Debug, Error, Diagnostic)]
pub enum SolveGroupUnsat {
    #[error("'{name}' is locked as a conda package but only requested by pypi dependencies")]
    CondaPackageShouldBePypi { name: String },
}

impl From<SourceRecordError> for Box<PlatformUnsat> {
    fn from(e: SourceRecordError) -> Self {
        match e {
            SourceRecordError::PackageNotProvided(ref e) => {
                Box::new(PlatformUnsat::SourcePackageNotFoundInMetadata {
                    package_name: e.name.as_source().to_string(),
                    manifest_path: e.pinned_source.to_string(),
                })
            }
            SourceRecordError::NoMatchingVariant {
                package,
                manifest_path,
                available,
            } => Box::new(PlatformUnsat::NoMatchingSourcePackageInMetadata {
                package,
                manifest_path,
                available,
            }),
            other => Box::new(PlatformUnsat::SourceRecord(other)),
        }
    }
}

impl PlatformUnsat {
    /// Returns true if this is a problem with pypi packages only. This means
    /// the conda packages are still considered valid.
    pub(crate) fn is_pypi_only(&self) -> bool {
        matches!(
            self,
            PlatformUnsat::UnsatisfiableRequirement(_, _)
                | PlatformUnsat::TooManyPypiPackages(_)
                | PlatformUnsat::AsPep508Error(_, _)
                | PlatformUnsat::FailedToDetermineSourceTreeHash(_, _)
                | PlatformUnsat::PythonVersionMismatch(_, _, _)
                | PlatformUnsat::SourceTreeHashMismatch(..)
                | PlatformUnsat::LocalPackageMetadataMismatch(_, _)
                | PlatformUnsat::FailedToReadLocalMetadata(_, _),
        )
    }
}
