pub mod pypi_metadata;

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fmt::{Display, Formatter},
    hash::Hash,
    path::{Path, PathBuf},
    str::FromStr,
    sync::LazyLock,
};

use futures::stream::{FuturesUnordered, StreamExt};
use itertools::{Either, Itertools};
use miette::Diagnostic;
use once_cell::sync::OnceCell;
use pep440_rs::VersionSpecifiers;
use pixi_build_discovery::EnabledProtocols;
use pixi_command_dispatcher::{
    BuildBackendMetadataSpec, BuildEnvironment, CommandDispatcher, CommandDispatcherError,
    DevSourceMetadataError, DevSourceMetadataSpec, SourceCheckoutError, SourceMetadataError,
    SourceMetadataSpec,
};
use pixi_config::Config;
use pixi_git::url::RepositoryUrl;
use pixi_manifest::{
    FeaturesExt,
    pypi::pypi_options::{NoBuild, PrereleaseMode},
};
use pixi_record::{
    DevSourceRecord, LockedGitUrl, ParseLockFileError, PinnedSourceSpec, PixiRecord,
    SourceMismatchError, SourceRecord, VariantValue,
};
use pixi_spec::{PixiSpec, SourceAnchor, SourceLocationSpec, SourceSpec, SpecConversionError};
use pixi_utils::variants::VariantConfig;
use pixi_uv_conversions::{
    AsPep508Error, as_uv_req, into_pixi_reference, pep508_requirement_to_uv_requirement,
    to_normalize, to_uv_specifiers, to_uv_version,
};
use pypi_modifiers::pypi_marker_env::determine_marker_environment;
use rattler_conda_types::{
    ChannelUrl, GenericVirtualPackage, MatchSpec, Matches, NamedChannelOrUrl, PackageName,
    PackageRecord, ParseChannelError, ParseMatchSpecError, ParseStrictness::Lenient, Platform,
};
use rattler_lock::{LockedPackageRef, PackageHashes, PypiIndexes, PypiPackageData, UrlOrPath};
use thiserror::Error;
use typed_path::Utf8TypedPathBuf;
use url::Url;
use uv_configuration::RAYON_INITIALIZE;
use uv_distribution_filename::{DistExtension, ExtensionError, SourceDistExtension};
use uv_distribution_types::{RequirementSource, RequiresPython};
use uv_git_types::GitReference;
use uv_pypi_types::{ParsedUrlError, PyProjectToml};

use super::{
    CondaPrefixUpdater, PixiRecordsByName, PypiRecord, PypiRecordsByName,
    outdated::{BuildCacheKey, EnvironmentBuildCache},
    package_identifier::ConversionError,
    resolve::build_dispatch::{LazyBuildDispatch, UvBuildDispatchParams},
};
use crate::workspace::{
    Environment, EnvironmentVars, HasWorkspaceRef, errors::VariantsError,
    grouped_environment::GroupedEnvironment,
};
use pixi_manifest::EnvironmentName;
use pixi_uv_context::UvResolutionContext;
use pixi_uv_conversions::{
    configure_insecure_hosts_for_tls_bypass, pypi_options_to_build_options,
    pypi_options_to_index_locations, to_index_strategy,
};
use pypi_modifiers::pypi_tags::{get_pypi_tags, is_python_record};
use std::sync::Arc;
use uv_client::{BaseClientBuilder, Connectivity, FlatIndexClient, RegistryClientBuilder};
use uv_distribution::DistributionDatabase;
use uv_distribution_types::{
    ConfigSettings, DependencyMetadata, DirectorySourceDist, Dist, HashPolicy, IndexUrl, SourceDist,
};
use uv_pep508;
use uv_resolver::FlatIndex;

#[derive(Debug, Error, Diagnostic)]
pub enum EnvironmentUnsat {
    #[error("the channels in the lock-file do not match the environments channels")]
    ChannelsMismatch,

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

    #[error(transparent)]
    ExcludeNewerMismatch(#[from] ExcludeNewerMismatch),
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
    locked_exclude_newer: Option<chrono::DateTime<chrono::Utc>>,
    expected_exclude_newer: Option<chrono::DateTime<chrono::Utc>>,
}

impl Display for ExcludeNewerMismatch {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match (self.locked_exclude_newer, self.expected_exclude_newer) {
            (Some(locked), None) => {
                write!(
                    f,
                    "the lock-file was solved with exclude-newer set to {locked}, but the environment does not have this option set"
                )
            }
            (None, Some(expected)) => {
                write!(
                    f,
                    "the lock-file was solved without exclude-newer, but the environment has this option set to {expected}"
                )
            }
            (Some(locked), Some(expected)) if locked != expected => {
                write!(
                    f,
                    "the lock-file was solved with exclude-newer set to {locked}, but the environment has this option set to {expected}"
                )
            }
            _ => unreachable!("if we get here the values are the same"),
        }
    }
}

#[derive(Debug, Error)]
pub struct IndexesMismatch {
    current: PypiIndexes,
    previous: Option<PypiIndexes>,
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
    #[error("dependencies changed - added: {added:?}, removed: {removed:?}")]
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

#[derive(Debug, Error, Diagnostic)]
pub enum PlatformUnsat {
    #[error("the requirement '{0}' could not be satisfied (required by '{1}')")]
    UnsatisfiableMatchSpec(Box<MatchSpec>, String),

    #[error("no package named exists '{0}' (required by '{1}')")]
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

    #[error("failed to build metadata for local package '{0}': {1}")]
    FailedToBuildLocalMetadata(pep508_rs::PackageName, String),

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

    #[error(transparent)]
    InvalidChannel(#[from] ParseChannelError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    DevSourceMetadataError(DevSourceMetadataError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    SourceCheckout(#[from] CommandDispatcherError<SourceCheckoutError>),

    #[error(transparent)]
    #[diagnostic(transparent)]
    DevSourceMetadata(#[from] CommandDispatcherError<DevSourceMetadataError>),

    #[error(transparent)]
    #[diagnostic(transparent)]
    SourceMetadata(#[from] CommandDispatcherError<SourceMetadataError>),

    #[error(
        "the locked package build source for '{0}' does not match the requested build source, {1}"
    )]
    PackageBuildSourceMismatch(String, SourceMismatchError),

    #[error("the locked metadata of '{0}' package changed (see trace logs for details)")]
    SourcePackageMetadataChanged(String),

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
}

#[derive(Debug, Error, Diagnostic)]
pub enum SolveGroupUnsat {
    #[error("'{name}' is locked as a conda package but only requested by pypi dependencies")]
    CondaPackageShouldBePypi { name: String },
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

/// Verifies that all the requirements of the specified `environment` can be
/// satisfied with the packages present in the lock-file.
///
/// This function returns a [`EnvironmentUnsat`] error if a verification issue
/// occurred. The [`EnvironmentUnsat`] error should contain enough information
/// for the user and developer to figure out what went wrong.
pub fn verify_environment_satisfiability(
    environment: &Environment<'_>,
    locked_environment: rattler_lock::Environment<'_>,
) -> Result<(), EnvironmentUnsat> {
    let grouped_env = GroupedEnvironment::from(environment.clone());

    // Check if the channels in the lock file match our current configuration. Note
    // that the order matters here. If channels are added in a different order,
    // the solver might return a different result.
    let config = environment.channel_config();
    let channels: Vec<ChannelUrl> = grouped_env
        .channels()
        .into_iter()
        .map(|channel| channel.clone().into_base_url(&config))
        .try_collect()?;

    let locked_channels: Vec<ChannelUrl> = locked_environment
        .channels()
        .iter()
        .map(|c| {
            NamedChannelOrUrl::from_str(&c.url)
                .unwrap_or_else(|_err| NamedChannelOrUrl::Name(c.url.clone()))
                .into_base_url(&config)
        })
        .try_collect()?;
    if !channels.eq(&locked_channels) {
        return Err(EnvironmentUnsat::ChannelsMismatch);
    }

    let platforms = environment.platforms();
    let locked_platforms = locked_environment.platforms().collect::<HashSet<_>>();
    let additional_platforms = locked_platforms
        .difference(&platforms)
        .map(|p| p.to_owned())
        .collect::<HashSet<_>>();
    if !additional_platforms.is_empty() {
        return Err(EnvironmentUnsat::AdditionalPlatformsInLockFile(
            additional_platforms,
        ));
    }

    // Do some more checks if we have pypi dependencies
    // 1. Check if the PyPI indexes are present and match
    // 2. Check if we have a no-build option set, that we only have binary packages,
    //    or an editable source
    if !environment.pypi_dependencies(None).is_empty() {
        let group_pypi_options = grouped_env.pypi_options();
        let indexes = rattler_lock::PypiIndexes::from(group_pypi_options.clone());

        // Check if the indexes in the lock file match our current configuration.
        verify_pypi_indexes(locked_environment, indexes)?;

        // Check that if `no-build` is set, we only have binary packages
        // or that the package that we disallow are not built from source
        if let Some(no_build) = group_pypi_options.no_build.as_ref() {
            verify_pypi_no_build(no_build, locked_environment)?;
        }
    }

    // Verify solver options
    let expected_solve_strategy = environment.solve_strategy().into();
    if locked_environment.solve_options().strategy != expected_solve_strategy {
        return Err(EnvironmentUnsat::SolveStrategyMismatch {
            locked_strategy: locked_environment.solve_options().strategy,
            expected_strategy: expected_solve_strategy,
        });
    }

    let expected_channel_priority = environment
        .channel_priority()
        .unwrap_or_default()
        .unwrap_or_default()
        .into();
    if locked_environment.solve_options().channel_priority != expected_channel_priority {
        return Err(EnvironmentUnsat::ChannelPriorityMismatch {
            locked_priority: locked_environment.solve_options().channel_priority,
            expected_priority: expected_channel_priority,
        });
    }

    let locked_prerelease_mode = locked_environment
        .solve_options()
        .pypi_prerelease_mode
        .unwrap_or_default()
        .into();
    let expected_prerelease_mode = grouped_env
        .pypi_options()
        .prerelease_mode
        .unwrap_or_default();
    if locked_prerelease_mode != expected_prerelease_mode {
        return Err(EnvironmentUnsat::PypiPrereleaseModeMismatch {
            locked_mode: locked_prerelease_mode,
            expected_mode: expected_prerelease_mode,
        });
    }

    let locked_exclude_newer = locked_environment.solve_options().exclude_newer;
    let expected_exclude_newer = environment.exclude_newer();
    if locked_exclude_newer != expected_exclude_newer {
        return Err(EnvironmentUnsat::ExcludeNewerMismatch(
            ExcludeNewerMismatch {
                locked_exclude_newer,
                expected_exclude_newer,
            },
        ));
    }

    Ok(())
}

fn verify_pypi_no_build(
    no_build: &NoBuild,
    locked_environment: rattler_lock::Environment<'_>,
) -> Result<(), EnvironmentUnsat> {
    // Check if we are disallowing all source packages or only a subset
    #[derive(Eq, PartialEq)]
    enum Check {
        All,
        Packages(HashSet<pep508_rs::PackageName>),
    }

    let check = match no_build {
        // Ok, so we are allowed to build any source package
        NoBuild::None => return Ok(()),
        // We are not allowed to build any source package
        NoBuild::All => Check::All,
        // We are not allowed to build a subset of source packages
        NoBuild::Packages(hash_set) => {
            let packages = hash_set
                .iter()
                .filter_map(|name| pep508_rs::PackageName::new(name.to_string()).ok())
                .collect();
            Check::Packages(packages)
        }
    };

    // Small helper function to get the dist extension from a url
    fn pypi_dist_extension_from_url(url: &Url) -> Result<DistExtension, ExtensionError> {
        // Take the file name from the url
        let path = url
            .path_segments()
            .and_then(|mut s| s.next_back())
            .unwrap_or_default();
        // Convert the path to a dist extension
        DistExtension::from_path(Path::new(path))
    }

    // Determine if we do not accept non-wheels for all packages or only for a
    // subset Check all the currently locked packages if we are making any
    // violations
    for (_, packages) in locked_environment.pypi_packages_by_platform() {
        for (package, _) in packages {
            let extension = match &package.location {
                // Get the extension from the url
                UrlOrPath::Url(url) => {
                    if url.scheme().starts_with("git+") {
                        // Just choose some source extension, does not really matter, cause it is
                        // actually a directory, this is just for the check
                        Ok(DistExtension::Source(SourceDistExtension::TarGz))
                    } else {
                        pypi_dist_extension_from_url(url)
                    }
                }
                UrlOrPath::Path(path) => {
                    let path = Path::new(path.as_str());
                    if path.is_dir() {
                        // Editables are allowed with no-build
                        if package.editable {
                            continue;
                        } else {
                            // Non-editable source packages might not be allowed
                            Ok(DistExtension::Source(SourceDistExtension::TarGz))
                        }
                    } else {
                        // Could be a reference to a wheel or sdist
                        DistExtension::from_path(path)
                    }
                }
            }?;

            match extension {
                // Wheels are fine
                DistExtension::Wheel => continue,
                // Check if we have a source package that we are not allowed to build
                // it could be that we are only disallowing for certain source packages
                DistExtension::Source(_) => match check {
                    Check::All => {
                        return Err(EnvironmentUnsat::NoBuildWithNonBinaryPackages(
                            package.name.to_string(),
                        ));
                    }
                    Check::Packages(ref hash_set) => {
                        if hash_set.contains(&package.name) {
                            return Err(EnvironmentUnsat::NoBuildWithNonBinaryPackages(
                                package.name.to_string(),
                            ));
                        }
                    }
                },
            }
        }
    }
    Ok(())
}

fn verify_pypi_indexes(
    locked_environment: rattler_lock::Environment<'_>,
    indexes: PypiIndexes,
) -> Result<(), EnvironmentUnsat> {
    match locked_environment.pypi_indexes() {
        None => {
            // Mismatch when there should be an index but there is not
            if locked_environment
                .lock_file()
                .version()
                .should_pypi_indexes_be_present()
                && locked_environment
                    .pypi_packages_by_platform()
                    .any(|(_platform, mut packages)| packages.next().is_some())
            {
                return Err(IndexesMismatch {
                    current: indexes,
                    previous: None,
                }
                .into());
            }
        }
        Some(locked_indexes) => {
            if locked_indexes != &indexes {
                return Err(IndexesMismatch {
                    current: indexes,
                    previous: Some(locked_indexes.clone()),
                }
                .into());
            }
        }
    }
    Ok(())
}

/// Context for verifying platform satisfiability.
pub struct VerifySatisfiabilityContext<'a> {
    pub environment: &'a Environment<'a>,
    pub command_dispatcher: CommandDispatcher,
    pub platform: Platform,
    pub project_root: &'a Path,
    pub uv_context: &'a OnceCell<UvResolutionContext>,
    pub config: &'a Config,
    pub project_env_vars: HashMap<EnvironmentName, EnvironmentVars>,
    pub build_caches: &'a mut HashMap<BuildCacheKey, Arc<EnvironmentBuildCache>>,
    /// Cache for static metadata extracted from pyproject.toml files.
    /// This is shared across platforms since static metadata is platform-independent.
    pub static_metadata_cache: &'a mut HashMap<PathBuf, pypi_metadata::LocalPackageMetadata>,
}

/// Verifies that the package requirements of the specified `environment` can be
/// satisfied with the packages present in the lock-file.
///
/// Both Conda and pypi packages are verified by this function. First all the
/// conda package are verified and then all the pypi packages are verified. This
/// is done so that if we can check if we only need to update the pypi
/// dependencies or also the conda dependencies.
///
/// This function returns a [`PlatformUnsat`] error if a verification issue
/// occurred. The [`PlatformUnsat`] error should contain enough information for
/// the user and developer to figure out what went wrong.
///
pub async fn verify_platform_satisfiability(
    ctx: &mut VerifySatisfiabilityContext<'_>,
    locked_environment: rattler_lock::Environment<'_>,
) -> Result<VerifiedIndividualEnvironment, Box<PlatformUnsat>> {
    // Convert the lock file into a list of conda and pypi packages
    let mut pixi_records: Vec<PixiRecord> = Vec::new();
    let mut pypi_packages: Vec<PypiRecord> = Vec::new();
    for package in locked_environment
        .packages(ctx.platform)
        .into_iter()
        .flatten()
    {
        match package {
            LockedPackageRef::Conda(conda) => {
                let url = conda.location().clone();
                pixi_records.push(
                    PixiRecord::from_conda_package_data(conda.clone(), ctx.project_root)
                        .map_err(|e| PlatformUnsat::CorruptedEntry(url.to_string(), e))?,
                );
            }
            LockedPackageRef::Pypi(pypi, env) => {
                pypi_packages.push((pypi.clone(), env.clone()));
            }
        }
    }

    // to reflect new purls for pypi packages
    // we need to invalidate the locked environment
    // if all conda packages have empty purls
    if ctx.environment.has_pypi_dependencies()
        && pypi_packages.is_empty()
        && pixi_records
            .iter()
            .filter_map(PixiRecord::as_binary)
            .all(|record| record.package_record.purls.is_none())
    {
        {
            return Err(Box::new(PlatformUnsat::MissingPurls));
        }
    }

    // Create a lookup table from package name to package record. Returns an error
    // if we find a duplicate entry for a record
    let pixi_records_by_name = match PixiRecordsByName::from_unique_iter(pixi_records.clone()) {
        Ok(pixi_records) => pixi_records,
        Err(duplicate) => {
            return Err(Box::new(PlatformUnsat::DuplicateEntry(
                duplicate.package_record().name.as_source().to_string(),
            )));
        }
    };

    // Create a lookup table from package name to package record. Returns an error
    // if we find a duplicate entry for a record
    let pypi_records_by_name = match PypiRecordsByName::from_unique_iter(pypi_packages) {
        Ok(pypi_packages) => pypi_packages,
        Err(duplicate) => {
            return Err(Box::new(PlatformUnsat::DuplicateEntry(
                duplicate.0.name.to_string(),
            )));
        }
    };

    // Get host platform records for building (we can only run Python on the host platform)
    let best_platform = ctx.environment.best_platform();
    let building_pixi_records = if ctx.platform == best_platform {
        // Same platform, reuse the records
        Ok(pixi_records_by_name.clone())
    } else {
        // Different platform - extract host platform records for building
        let mut host_pixi_records: Vec<PixiRecord> = Vec::new();
        for package in locked_environment
            .packages(best_platform)
            .into_iter()
            .flatten()
        {
            if let LockedPackageRef::Conda(conda) = package {
                let url = conda.location().clone();
                host_pixi_records.push(
                    PixiRecord::from_conda_package_data(conda.clone(), ctx.project_root)
                        .map_err(|e| PlatformUnsat::CorruptedEntry(url.to_string(), e))?,
                );
            }
        }
        PixiRecordsByName::from_unique_iter(host_pixi_records).map_err(|duplicate| {
            PlatformUnsat::DuplicateEntry(duplicate.package_record().name.as_source().to_string())
        })
    };

    // Run satisfiability check - for local packages with dynamic metadata,
    // we use UV infrastructure to build metadata if available.
    verify_package_platform_satisfiability(
        ctx,
        &pixi_records_by_name,
        &pypi_records_by_name,
        building_pixi_records,
    )
    .await
}

#[allow(clippy::large_enum_variant)]
/// A dependency that needs to be checked in the lock file
pub enum Dependency {
    Input(PackageName, PixiSpec, Cow<'static, str>),
    Conda(MatchSpec, Cow<'static, str>),
    CondaSource(PackageName, MatchSpec, SourceSpec, Cow<'static, str>),
    PyPi(uv_distribution_types::Requirement, Cow<'static, str>),
}

impl Dependency {
    /// Extract the conda package name from this dependency, if it has one.
    /// Returns None for PyPi dependencies.
    pub fn conda_package_name(&self) -> Option<PackageName> {
        match self {
            Dependency::Input(name, _, _) => Some(name.clone()),
            Dependency::Conda(spec, _) => spec.name.as_ref().and_then(|m| m.as_exact().cloned()),
            Dependency::CondaSource(name, _, _, _) => Some(name.clone()),
            Dependency::PyPi(_, _) => None,
        }
    }
}

/// Check satatisfiability of a pypi requirement against a locked pypi package
/// This also does an additional check for git urls when using direct url
/// references
pub(crate) fn pypi_satifisfies_editable(
    spec: &uv_distribution_types::Requirement,
    locked_data: &PypiPackageData,
    project_root: &Path,
) -> Result<(), Box<PlatformUnsat>> {
    // We dont match on spec.is_editable() != locked_data.editable
    // as it will happen later in verify_package_platform_satisfiability
    // TODO: could be a potential refactoring opportunity

    match &spec.source {
        RequirementSource::Registry { .. }
        | RequirementSource::Url { .. }
        | RequirementSource::Path { .. }
        | RequirementSource::Git { .. } => {
            unreachable!(
                "editable requirement cannot be from registry, url, git or path (non-directory)"
            )
        }
        RequirementSource::Directory { install_path, .. } => match &locked_data.location {
            // If we have an url requirement locked, but the editable is requested, this does not
            // satifsfy
            UrlOrPath::Url(url) => Err(Box::new(PlatformUnsat::EditablePackageIsUrl(
                spec.name.clone(),
                url.to_string(),
            ))),
            UrlOrPath::Path(path) => {
                // Most of the times the path will be relative to the project root
                let absolute_path = if path.is_absolute() {
                    Cow::Borrowed(Path::new(path.as_str()))
                } else {
                    Cow::Owned(project_root.join(Path::new(path.as_str())))
                };
                // Absolute paths can have symbolic links, so we canonicalize
                let canocalized_path = dunce::canonicalize(&absolute_path).map_err(|e| {
                    Box::new(PlatformUnsat::FailedToCanonicalizePath(
                        absolute_path.to_path_buf(),
                        e,
                    ))
                })?;

                if canocalized_path != install_path.as_ref() {
                    return Err(Box::new(PlatformUnsat::EditablePackagePathMismatch(
                        spec.name.clone(),
                        absolute_path.into_owned(),
                        install_path.to_path_buf(),
                    )));
                }
                Ok(())
            }
        },
    }
}

/// Check satatisfiability of a pypi requirement against a locked pypi package
/// This also does an additional check for git urls when using direct url
/// references
pub(crate) fn pypi_satifisfies_requirement(
    spec: &uv_distribution_types::Requirement,
    locked_data: &PypiPackageData,
    project_root: &Path,
) -> Result<(), Box<PlatformUnsat>> {
    if spec.name.to_string() != locked_data.name.to_string() {
        return Err(PlatformUnsat::LockedPyPINamesMismatch {
            expected: spec.name.to_string(),
            found: locked_data.name.to_string(),
        }
        .into());
    }

    match &spec.source {
        RequirementSource::Registry { specifier, .. } => {
            // In the old way we always satisfy based on version so let's keep it similar
            // here
            let version_string = locked_data.version.to_string();
            if specifier.contains(
                &uv_pep440::Version::from_str(&version_string).expect("could not parse version"),
            ) {
                Ok(())
            } else {
                Err(PlatformUnsat::LockedPyPIVersionsMismatch {
                    name: spec.name.clone().to_string(),
                    specifiers: specifier.clone().to_string(),
                    version: version_string,
                }
                .into())
            }
        }
        RequirementSource::Url { url: spec_url, .. } => {
            if let UrlOrPath::Url(locked_url) = &locked_data.location {
                // Url may not start with git, and must start with direct+
                if locked_url.as_str().starts_with("git+")
                    || !locked_url.as_str().starts_with("direct+")
                {
                    return Err(PlatformUnsat::LockedPyPIMalformedUrl(locked_url.clone()).into());
                }
                let locked_url = locked_url
                    .as_ref()
                    .strip_prefix("direct+")
                    .and_then(|str| Url::parse(str).ok())
                    .unwrap_or(locked_url.clone());

                if *spec_url.raw() == locked_url.clone().into() {
                    return Ok(());
                } else {
                    return Err(PlatformUnsat::LockedPyPIDirectUrlMismatch {
                        name: spec.name.clone().to_string(),
                        spec_url: spec_url.raw().to_string(),
                        lock_url: locked_url.to_string(),
                    }
                    .into());
                }
            }
            Err(PlatformUnsat::LockedPyPIRequiresDirectUrl(spec.name.to_string()).into())
        }
        RequirementSource::Git {
            git, subdirectory, ..
        } => {
            let repository = git.repository();
            let reference = git.reference();
            match &locked_data.location {
                UrlOrPath::Url(url) => {
                    if let Ok(pinned_git_spec) = LockedGitUrl::new(url.clone()).to_pinned_git_spec()
                    {
                        let pinned_repository = RepositoryUrl::new(&pinned_git_spec.git);
                        let specified_repository = RepositoryUrl::new(repository);

                        let repo_is_same = pinned_repository == specified_repository;
                        if !repo_is_same {
                            return Err(PlatformUnsat::LockedPyPIGitUrlMismatch {
                                name: spec.name.clone().to_string(),
                                spec_url: repository.to_string(),
                                lock_url: pinned_git_spec.git.to_string(),
                            }
                            .into());
                        }
                        // If the spec does not specify a revision than any will do
                        // E.g `git.com/user/repo` is the same as `git.com/user/repo@adbdd`
                        if *reference == GitReference::DefaultBranch {
                            return Ok(());
                        }

                        if pinned_git_spec.source.subdirectory
                            != subdirectory
                                .as_ref()
                                .map(|s| s.to_string_lossy().to_string())
                        {
                            return Err(PlatformUnsat::LockedPyPIGitSubdirectoryMismatch {
                                name: spec.name.clone().to_string(),
                                spec_subdirectory: subdirectory
                                    .as_ref()
                                    .map_or_else(String::default, |s| {
                                        s.to_string_lossy().to_string()
                                    }),
                                lock_subdirectory: pinned_git_spec
                                    .source
                                    .subdirectory
                                    .unwrap_or_default(),
                            }
                            .into());
                        }
                        // If the spec does specify a revision than the revision must match
                        // convert first to the same type
                        let pixi_reference = into_pixi_reference(reference.clone());

                        if pinned_git_spec.source.reference == pixi_reference {
                            return Ok(());
                        } else {
                            return Err(PlatformUnsat::LockedPyPIGitRefMismatch {
                                name: spec.name.clone().to_string(),
                                expected_ref: reference.to_string(),
                                found_ref: pinned_git_spec.source.reference.to_string(),
                            }
                            .into());
                        }
                    }
                    Err(PlatformUnsat::LockedPyPIRequiresGitUrl(
                        spec.name.to_string(),
                        url.to_string(),
                    )
                    .into())
                }
                UrlOrPath::Path(path) => Err(PlatformUnsat::LockedPyPIRequiresGitUrl(
                    spec.name.to_string(),
                    path.to_string(),
                )
                .into()),
            }
        }
        RequirementSource::Path { install_path, .. }
        | RequirementSource::Directory { install_path, .. } => {
            if let UrlOrPath::Path(locked_path) = &locked_data.location {
                let install_path =
                    Utf8TypedPathBuf::from(install_path.to_string_lossy().to_string());
                let project_root =
                    Utf8TypedPathBuf::from(project_root.to_string_lossy().to_string());
                // Join relative paths with the project root
                let locked_path = if locked_path.is_absolute() {
                    locked_path.clone()
                } else {
                    project_root.join(locked_path.to_path()).normalize()
                };
                if locked_path.to_path() != install_path {
                    return Err(PlatformUnsat::LockedPyPIPathMismatch {
                        name: spec.name.clone().to_string(),
                        install_path: install_path.to_string(),
                        locked_path: locked_path.to_string(),
                    }
                    .into());
                }
                return Ok(());
            }
            Err(PlatformUnsat::LockedPyPIRequiresPath(spec.name.to_string()).into())
        }
    }
}

/// A struct that records some information about an environment that has been
/// verified.
///
/// Some of this information from an individual environment is useful to have
/// when considering solve groups.
pub struct VerifiedIndividualEnvironment {
    /// All packages in the environment that are expected to be conda packages
    /// e.g. they are in the environment as a direct or transitive dependency of
    /// another conda package.
    pub expected_conda_packages: HashSet<PackageName>,

    /// All conda packages that satisfy a pypi requirement.
    pub conda_packages_used_by_pypi: HashSet<PackageName>,
}

/// Verify that source packages in the lock file still match their current
/// metadata.
///
/// This function fetches the current metadata for each source package and
/// compares it with the locked metadata to detect if any source packages have
/// changed.
#[allow(clippy::too_many_arguments)]
async fn verify_source_metadata(
    source_records: Vec<&pixi_record::SourceRecord>,
    command_dispatcher: CommandDispatcher,
    channel_config: rattler_conda_types::ChannelConfig,
    channel_urls: Vec<ChannelUrl>,
    variants: std::collections::BTreeMap<String, Vec<VariantValue>>,
    variant_files: Vec<PathBuf>,
    virtual_packages: Vec<GenericVirtualPackage>,
    platform: Platform,
) -> Result<(), Box<PlatformUnsat>> {
    // Process all source records concurrently
    let mut results: FuturesUnordered<_> = source_records
        .into_iter()
        .map(|source_record| {
            let command_dispatcher = command_dispatcher.clone();
            let channel_config = channel_config.clone();
            let channel_urls = channel_urls.clone();
            let variants = variants.clone();
            let variant_files = variant_files.clone();
            let virtual_packages = virtual_packages.clone();

            async move {
                // Build source metadata spec to request current package metadata
                let source_metadata_spec = SourceMetadataSpec {
                    package: source_record.package_record.name.clone(),
                    backend_metadata: BuildBackendMetadataSpec {
                        manifest_source: source_record.manifest_source.clone(),
                        preferred_build_source: source_record.build_source.clone(),
                        channel_config,
                        channels: channel_urls,
                        build_environment: BuildEnvironment {
                            host_platform: platform,
                            build_platform: platform,
                            host_virtual_packages: virtual_packages.clone(),
                            build_virtual_packages: virtual_packages,
                        },
                        variant_configuration: Some(variants),
                        variant_files: Some(variant_files),
                        enabled_protocols: EnabledProtocols::default(),
                    },
                };

                // Request source metadata to verify if it its still matches the locked one
                let current_source_metadata = command_dispatcher
                    .source_metadata(source_metadata_spec)
                    .await
                    .map_err(|e| Box::new(PlatformUnsat::SourceMetadata(e)))?;

                if current_source_metadata.cached_metadata.records.is_empty() {
                    return Err(Box::new(PlatformUnsat::SourcePackageNotFoundInMetadata {
                        package_name: source_record.package_record.name.as_source().to_string(),
                        manifest_path: source_record
                            .manifest_source
                            .as_path()
                            .map(|p| p.path.to_string())
                            .unwrap_or_else(|| source_record.manifest_source.to_string()),
                    }));
                }

                // Find the record that matches our locked package name and build string.
                // When there are variants, there can be multiple source metadata entries
                // with the same package name, so we also match on the build string which
                // encodes the variant information.
                let current_records = &current_source_metadata.cached_metadata.records;
                let current_record = current_records
                    .iter()
                    .find(|r| source_record.refers_to_same_output(r));

                let Some(current_record) = current_record else {
                    let manifest_path = source_record
                        .manifest_source
                        .as_path()
                        .map(|p| p.path.to_string())
                        .unwrap_or_else(|| source_record.manifest_source.to_string());
                    return Err(Box::new(PlatformUnsat::NoMatchingSourcePackageInMetadata {
                        package: format_source_record(source_record),
                        manifest_path,
                        available: current_records
                            .iter()
                            .map(format_source_record)
                            .format(", ")
                            .to_string(),
                    }));
                };

                // Check if the build source location changed
                if current_record.build_source != source_record.build_source {
                    return Err(Box::new(PlatformUnsat::SourceBuildLocationChanged(
                        source_record.package_record.name.as_source().to_string(),
                        source_record
                            .build_source
                            .as_ref()
                            .map(|s| s.to_string())
                            .unwrap_or_default(),
                        current_record
                            .build_source
                            .as_ref()
                            .map(|s| s.to_string())
                            .unwrap_or_default(),
                    )));
                }

                // Check if the source dependencies match
                let package_name = source_record.package_record.name.as_source().to_string();
                for (source_name, locked_source_spec) in &source_record.sources {
                    match current_record.sources.get(source_name) {
                        Some(current_source_spec) => {
                            if locked_source_spec != current_source_spec {
                                return Err(Box::new(PlatformUnsat::SourceDependencyChanged {
                                    package: package_name,
                                    dependency: source_name.clone(),
                                    locked: locked_source_spec.to_string(),
                                    current: current_source_spec.to_string(),
                                }));
                            }
                        }
                        None => {
                            return Err(Box::new(PlatformUnsat::SourceDependencyChanged {
                                package: package_name,
                                dependency: source_name.clone(),
                                locked: locked_source_spec.to_string(),
                                current: "(removed)".to_string(),
                            }));
                        }
                    }
                }

                // Check if there are any new sources in current that weren't in locked
                for (source_name, current_source_spec) in &current_record.sources {
                    if !source_record.sources.contains_key(source_name) {
                        return Err(Box::new(PlatformUnsat::SourceDependencyChanged {
                            package: package_name.clone(),
                            dependency: source_name.clone(),
                            locked: "(not present)".to_string(),
                            current: current_source_spec.to_string(),
                        }));
                    }
                }

                // Check if the package record metadata matches
                let package_name = source_record.package_record.name.as_source();
                tracing::trace!(
                    "Checking package record equality for '{}' (current vs locked)",
                    package_name
                );

                if !package_records_are_equal(
                    &current_record.package_record,
                    &source_record.package_record,
                ) {
                    return Err(Box::new(PlatformUnsat::SourcePackageMetadataChanged(
                        package_name.to_string(),
                    )));
                }

                Ok(())
            }
        })
        .collect();

    // Check results and fail fast on first error
    while let Some(result) = results.next().await {
        result?;
    }

    Ok(())
}

/// Returns true if the package records are considered equal.
fn package_records_are_equal(a: &PackageRecord, b: &PackageRecord) -> bool {
    // Use destructuring to ensure we get compiler errors if these types change significantly.
    let PackageRecord {
        arch: _,
        build: a_build,
        build_number: a_build_number,
        constrains: a_constrains,
        depends: a_depends,
        experimental_extra_depends: a_extra_depends,
        features: a_features,
        legacy_bz2_md5: _,
        legacy_bz2_size: _,
        license: a_license,
        license_family: a_license_family,
        md5: _,
        name: a_name,
        noarch: a_noarch,
        platform: _,
        purls: a_purls,
        python_site_packages_path: a_python_site_packages_path,
        run_exports: a_run_exports,
        sha256: _,
        size: _,
        subdir: a_subdir,
        timestamp: _,
        track_features: a_track_features,
        version: a_version,
    } = &a;
    let PackageRecord {
        arch: _,
        build: b_build,
        build_number: b_build_number,
        constrains: b_constrains,
        depends: b_depends,
        experimental_extra_depends: b_extra_depends,
        features: b_features,
        legacy_bz2_md5: _,
        legacy_bz2_size: _,
        license: b_license,
        license_family: b_license_family,
        md5: _,
        name: b_name,
        noarch: b_noarch,
        platform: _,
        purls: b_purls,
        python_site_packages_path: b_python_site_packages_path,
        run_exports: b_run_exports,
        sha256: _,
        size: _,
        subdir: b_subdir,
        timestamp: _,
        track_features: b_track_features,
        version: b_version,
    } = &b;

    a_build == b_build
        && a_build_number == b_build_number
        && a_constrains == b_constrains
        && a_depends == b_depends
        && a_extra_depends == b_extra_depends
        && a_features == b_features
        && a_license == b_license
        && a_license_family == b_license_family
        && a_name == b_name
        && a_noarch == b_noarch
        && a_purls == b_purls
        && a_python_site_packages_path == b_python_site_packages_path
        && match (a_run_exports, b_run_exports) {
            (Some(a_run_exports), Some(b_run_exports)) => a_run_exports == b_run_exports,
            (Some(a_run_exports), None) => a_run_exports.is_empty(),
            (None, Some(b_run_exports)) => b_run_exports.is_empty(),
            (None, None) => true,
        }
        && a_subdir == b_subdir
        && a_track_features == b_track_features
        && a_version == b_version
}

fn format_source_record(r: &SourceRecord) -> String {
    let variants = r.variants.as_ref().map(|v| {
        format!(
            "[{}]",
            v.iter()
                .format_with(", ", |(k, v), f| f(&format_args!("{k}={v}")))
        )
    });
    format!(
        "{}/{}={}={} {}",
        &r.package_record.subdir,
        r.package_record.name.as_source(),
        &r.package_record.version,
        &r.package_record.build,
        variants.unwrap_or_default()
    )
}

/// Resolve dev dependencies and get all their dependencies
pub async fn resolve_dev_dependencies(
    dev_dependencies: Vec<(PackageName, SourceSpec)>,
    command_dispatcher: &CommandDispatcher,
    channel_config: &rattler_conda_types::ChannelConfig,
    channels: &[ChannelUrl],
    build_environment: &BuildEnvironment,
    variants: &std::collections::BTreeMap<String, Vec<VariantValue>>,
    variant_files: &[PathBuf],
) -> Result<Vec<Dependency>, PlatformUnsat> {
    let futures = dev_dependencies
        .into_iter()
        .map(|(package_name, source_spec)| {
            let command_dispatcher = command_dispatcher.clone();
            let channel_config = channel_config.clone();
            let channels = channels.to_vec();
            let build_environment = build_environment.clone();
            let variants = variants.clone();
            let variant_files = variant_files.to_vec();

            resolve_single_dev_dependency(
                package_name,
                source_spec,
                command_dispatcher,
                channel_config,
                channels,
                build_environment,
                variants,
                variant_files,
            )
        })
        .collect::<futures::stream::FuturesUnordered<_>>();

    let results: Vec<Result<Vec<Dependency>, PlatformUnsat>> = futures.collect().await;

    let mut resolved_dependencies = Vec::new();
    for result in results {
        resolved_dependencies.extend(result?);
    }

    Ok(resolved_dependencies)
}

/// Resolves all dependencies of a single dev dependency
#[allow(clippy::too_many_arguments)]
async fn resolve_single_dev_dependency(
    package_name: PackageName,
    source_spec: SourceSpec,
    command_dispatcher: CommandDispatcher,
    channel_config: rattler_conda_types::ChannelConfig,
    channels: Vec<ChannelUrl>,
    build_environment: BuildEnvironment,
    variants: std::collections::BTreeMap<String, Vec<VariantValue>>,
    variant_files: Vec<PathBuf>,
) -> Result<Vec<Dependency>, PlatformUnsat> {
    let pinned_source = command_dispatcher
        .pin_and_checkout(source_spec.location)
        .await?;

    // Create the spec for getting dev source metadata
    let spec = DevSourceMetadataSpec {
        package_name: package_name.clone(),
        backend_metadata: BuildBackendMetadataSpec {
            manifest_source: pinned_source.pinned,
            preferred_build_source: None,
            channel_config: channel_config.clone(),
            channels,
            build_environment,
            variant_configuration: Some(variants),
            variant_files: Some(variant_files),
            enabled_protocols: Default::default(),
        },
    };

    let dev_metadata = command_dispatcher.dev_source_metadata(spec).await?;

    let dev_deps = DevSourceRecord::dev_source_dependencies(&dev_metadata.records);

    let (dev_source, dev_bin) =
        DevSourceRecord::split_into_source_and_binary_requirements(dev_deps);

    let mut dependencies = Vec::new();

    // Process source dependencies
    for (dev_name, dep) in dev_source.into_specs() {
        let anchored_source = SourceAnchor::Workspace.resolve(dep.clone().location);

        let spec = MatchSpec::from_str(dev_name.as_source(), Lenient).map_err(|e| {
            PlatformUnsat::FailedToParseMatchSpec(dev_name.as_source().to_string(), e)
        })?;

        dependencies.push(Dependency::CondaSource(
            dev_name.clone(),
            spec,
            SourceSpec {
                location: anchored_source,
            },
            Cow::Owned(format!("{} @ {}", dev_name.as_source(), dep)),
        ));
    }

    // Process binary dependencies
    for (dev_name, binary_spec) in dev_bin.into_specs() {
        // Convert BinarySpec to NamelessMatchSpec
        let nameless_spec = binary_spec
            .try_into_nameless_match_spec(&channel_config)
            .map_err(|e| {
                let parse_channel_err: ParseMatchSpecError = match e {
                    SpecConversionError::NonAbsoluteRootDir(p) => {
                        ParseChannelError::NonAbsoluteRootDir(p).into()
                    }
                    SpecConversionError::NotUtf8RootDir(p) => {
                        ParseChannelError::NotUtf8RootDir(p).into()
                    }
                    SpecConversionError::InvalidPath(p) => ParseChannelError::InvalidPath(p).into(),
                    SpecConversionError::InvalidChannel(_name, p) => p.into(),
                    SpecConversionError::MissingName => ParseMatchSpecError::MissingPackageName,
                };
                PlatformUnsat::FailedToParseMatchSpec(
                    dev_name.as_source().to_string(),
                    parse_channel_err,
                )
            })?;

        let spec = MatchSpec::from_nameless(nameless_spec, Some(dev_name.clone().into()));

        dependencies.push(Dependency::Conda(
            spec,
            Cow::Owned(dev_name.as_source().to_string()),
        ));
    }

    Ok(dependencies)
}

pub(crate) async fn verify_package_platform_satisfiability(
    ctx: &mut VerifySatisfiabilityContext<'_>,
    locked_pixi_records: &PixiRecordsByName,
    locked_pypi_environment: &PypiRecordsByName,
    building_pixi_records: Result<PixiRecordsByName, PlatformUnsat>,
) -> Result<VerifiedIndividualEnvironment, Box<PlatformUnsat>> {
    // Determine the dependencies requested by the environment
    let environment_dependencies = ctx
        .environment
        .combined_dependencies(Some(ctx.platform))
        .into_specs()
        .map(|(package_name, spec)| Dependency::Input(package_name, spec, "<environment>".into()))
        .collect_vec();

    // Get the dev dependencies for this platform
    let dev_dependencies = ctx
        .environment
        .combined_dev_dependencies(Some(ctx.platform))
        .into_specs()
        .collect_vec();

    // retrieve dependency-overrides
    // map it to (name => requirement) for later matching
    let dependency_overrides = ctx
        .environment
        .pypi_options()
        .dependency_overrides
        .unwrap_or_default()
        .into_iter()
        .map(|(name, req)| -> Result<_, Box<PlatformUnsat>> {
            let uv_req = as_uv_req(&req, name.as_source(), ctx.project_root).map_err(|e| {
                Box::new(PlatformUnsat::AsPep508Error(
                    name.as_normalized().clone(),
                    e,
                ))
            })?;
            Ok((uv_req.name.clone(), uv_req))
        })
        .collect::<Result<indexmap::IndexMap<_, _>, _>>()?;

    // Transform from PyPiPackage name into UV Requirement type
    let project_root = ctx.project_root;
    let pypi_requirements = ctx
        .environment
        .pypi_dependencies(Some(ctx.platform))
        .iter()
        .flat_map(|(name, reqs)| {
            reqs.iter().map(move |req| {
                Ok::<Dependency, Box<PlatformUnsat>>(Dependency::PyPi(
                    as_uv_req(req, name.as_source(), project_root).map_err(|e| {
                        Box::new(PlatformUnsat::AsPep508Error(
                            name.as_normalized().clone(),
                            e,
                        ))
                    })?,
                    "<environment>".into(),
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    if pypi_requirements.is_empty() && !locked_pypi_environment.is_empty() {
        return Err(Box::new(PlatformUnsat::TooManyPypiPackages(
            locked_pypi_environment.names().cloned().collect(),
        )));
    }

    // Create a list of virtual packages by name
    let virtual_packages = ctx
        .environment
        .virtual_packages(ctx.platform)
        .into_iter()
        .map(GenericVirtualPackage::from)
        .map(|vpkg| (vpkg.name.clone(), vpkg))
        .collect::<HashMap<_, _>>();

    // The list of channels and platforms we need for this task
    let channels = ctx
        .environment
        .channels()
        .into_iter()
        .cloned()
        .collect_vec();

    // Get the channel configuration
    let channel_config = ctx.environment.workspace().channel_config();

    // Resolve the channel URLs for the channels we need.
    let channels = channels
        .iter()
        .map(|c| c.clone().into_base_url(&channel_config))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| Box::new(PlatformUnsat::InvalidChannel(e)))?;

    // Determine the build variants
    let VariantConfig {
        variant_configuration,
        variant_files,
    } = ctx
        .environment
        .workspace()
        .variants(ctx.platform)
        .map_err(|e| Box::new(PlatformUnsat::Variants(e)))?;

    let build_environment =
        BuildEnvironment::simple(ctx.platform, virtual_packages.values().cloned().collect());

    // Get all source records from the lock file for metadata verification
    let source_records: Vec<_> = locked_pixi_records
        .records
        .iter()
        .filter_map(PixiRecord::as_source)
        .collect();

    // Resolve dev dependencies and verify source metadata in parallel
    let dev_deps_future = resolve_dev_dependencies(
        dev_dependencies,
        &ctx.command_dispatcher,
        &channel_config,
        &channels,
        &build_environment,
        &variant_configuration,
        &variant_files,
    );

    let source_metadata_future = verify_source_metadata(
        source_records,
        ctx.command_dispatcher.clone(),
        channel_config.clone(),
        channels.clone(),
        variant_configuration.clone(),
        variant_files.clone(),
        virtual_packages.values().cloned().collect(),
        ctx.platform,
    );

    let (resolved_dev_dependencies, source_metadata_result) =
        futures::join!(dev_deps_future, source_metadata_future);

    let resolved_dev_dependencies = resolved_dev_dependencies?;
    source_metadata_result?;

    if (environment_dependencies.is_empty() && resolved_dev_dependencies.is_empty())
        && !locked_pixi_records.is_empty()
    {
        return Err(Box::new(PlatformUnsat::TooManyCondaPackages(Vec::new())));
    }

    // Find the python interpreter from the list of conda packages. Note that this
    // refers to the locked python interpreter, it might not match the specs
    // from the environment. That is ok because we will find that out when we
    // check all the records.
    let python_interpreter_record = locked_pixi_records.python_interpreter_record();

    // Determine the marker environment from the python interpreter package.
    let marker_environment = python_interpreter_record
        .map(|interpreter| determine_marker_environment(ctx.platform, &interpreter.package_record))
        .transpose()
        .map_err(|err| {
            Box::new(PlatformUnsat::FailedToDetermineMarkerEnvironment(
                err.into(),
            ))
        });

    // We cannot determine the marker environment, for example if installing
    // `wasm32` dependencies. However, it also doesn't really matter if we don't
    // have any pypi requirements.
    let marker_environment = match marker_environment {
        Err(err) => {
            if !pypi_requirements.is_empty() {
                return Err(err);
            } else {
                None
            }
        }
        Ok(marker_environment) => marker_environment,
    };

    // Determine the pypi packages provided by the locked conda packages.
    let locked_conda_pypi_packages = locked_pixi_records
        .by_pypi_name()
        .map_err(From::from)
        .map_err(Box::new)?;

    // Keep a list of all conda packages that we have already visisted
    let mut conda_packages_visited = HashSet::new();
    let mut pypi_packages_visited = HashSet::new();
    let mut pypi_requirements_visited = pypi_requirements
        .iter()
        .filter_map(|r| match r {
            Dependency::PyPi(req, _) => Some(req.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();

    // Iterate over all packages. First iterate over all conda matchspecs and then
    // over all pypi requirements. We want to ensure we always check the conda
    // packages first.
    let mut conda_queue = environment_dependencies
        .into_iter()
        .chain(resolved_dev_dependencies.into_iter())
        .collect_vec();
    let mut pypi_queue = pypi_requirements;
    let mut expected_conda_source_dependencies = HashSet::new();
    let mut expected_conda_packages = HashSet::new();
    let mut conda_packages_used_by_pypi = HashSet::new();
    let mut delayed_pypi_error = None;

    while let Some(package) = conda_queue.pop().or_else(|| pypi_queue.pop()) {
        // Determine the package that matches the requirement of matchspec.
        let found_package = match package {
            Dependency::Input(name, spec, source) => {
                let found_package = match spec.into_source_or_binary() {
                    Either::Left(source_spec) => {
                        expected_conda_source_dependencies.insert(name.clone());
                        find_matching_source_package(
                            locked_pixi_records,
                            name,
                            source_spec,
                            source,
                            None,
                        )?
                    }
                    Either::Right(binary_spec) => {
                        let spec = match binary_spec.try_into_nameless_match_spec(&channel_config) {
                            Err(e) => {
                                let parse_channel_err: ParseMatchSpecError = match e {
                                    SpecConversionError::NonAbsoluteRootDir(p) => {
                                        ParseChannelError::NonAbsoluteRootDir(p).into()
                                    }
                                    SpecConversionError::NotUtf8RootDir(p) => {
                                        ParseChannelError::NotUtf8RootDir(p).into()
                                    }
                                    SpecConversionError::InvalidPath(p) => {
                                        ParseChannelError::InvalidPath(p).into()
                                    }
                                    SpecConversionError::InvalidChannel(_name, p) => p.into(),
                                    SpecConversionError::MissingName => {
                                        ParseMatchSpecError::MissingPackageName
                                    }
                                };
                                return Err(Box::new(PlatformUnsat::FailedToParseMatchSpec(
                                    name.as_source().to_string(),
                                    parse_channel_err,
                                )));
                            }
                            Ok(spec) => spec,
                        };
                        match find_matching_package(
                            locked_pixi_records,
                            &virtual_packages,
                            MatchSpec::from_nameless(spec, Some(name.into())),
                            source,
                        )? {
                            Some(pkg) => pkg,
                            None => continue,
                        }
                    }
                };

                expected_conda_packages
                    .insert(locked_pixi_records.records[found_package.0].name().clone());
                FoundPackage::Conda(found_package)
            }
            Dependency::Conda(spec, source) => {
                match find_matching_package(locked_pixi_records, &virtual_packages, spec, source)? {
                    Some(pkg) => {
                        expected_conda_packages
                            .insert(locked_pixi_records.records[pkg.0].name().clone());
                        FoundPackage::Conda(pkg)
                    }
                    None => continue,
                }
            }
            Dependency::CondaSource(name, spec, source_spec, source) => {
                expected_conda_source_dependencies.insert(name.clone());
                FoundPackage::Conda(find_matching_source_package(
                    locked_pixi_records,
                    name,
                    source_spec,
                    source,
                    Some(spec),
                )?)
            }
            Dependency::PyPi(requirement, source) => {
                // Check if there is a pypi identifier that matches our requirement.
                if let Some((identifier, repodata_idx, _)) =
                    locked_conda_pypi_packages.get(&requirement.name)
                {
                    if requirement.is_editable() {
                        delayed_pypi_error.get_or_insert_with(|| {
                            Box::new(PlatformUnsat::EditableDependencyOnCondaInstalledPackage(
                                requirement.name.clone(),
                                Box::new(requirement.source.clone()),
                            ))
                        });
                    }

                    if matches!(requirement.source, RequirementSource::Url { .. }) {
                        delayed_pypi_error.get_or_insert_with(|| {
                            Box::new(PlatformUnsat::DirectUrlDependencyOnCondaInstalledPackage(
                                requirement.name.clone(),
                            ))
                        });
                    }

                    if matches!(requirement.source, RequirementSource::Git { .. }) {
                        delayed_pypi_error.get_or_insert_with(|| {
                            Box::new(PlatformUnsat::GitDependencyOnCondaInstalledPackage(
                                requirement.name.clone(),
                            ))
                        });
                    }

                    // Use the overridden requirement if specified (e.g. for pytorch/torch)
                    let requirement_to_check = dependency_overrides
                        .get(&requirement.name)
                        .cloned()
                        .unwrap_or(requirement.clone());

                    if !identifier.satisfies(&requirement_to_check)? {
                        // The record does not match the spec, the lock-file is inconsistent.
                        delayed_pypi_error.get_or_insert_with(|| {
                            Box::new(PlatformUnsat::CondaUnsatisfiableRequirement(
                                Box::new(requirement.clone()),
                                source.into_owned(),
                            ))
                        });
                    }
                    let pkg_idx = CondaPackageIdx(*repodata_idx);
                    conda_packages_used_by_pypi
                        .insert(locked_pixi_records.records[pkg_idx.0].name().clone());
                    FoundPackage::Conda(pkg_idx)
                } else {
                    match to_normalize(&requirement.name)
                        .map(|name| locked_pypi_environment.index_by_name(&name))
                    {
                        Ok(Some(idx)) => {
                            let record = &locked_pypi_environment.records[idx];

                            // use the overridden requirements if specified
                            let requirement = dependency_overrides
                                .get(&requirement.name)
                                .cloned()
                                .unwrap_or(requirement);

                            if requirement.is_editable() {
                                if let Err(err) = pypi_satifisfies_editable(
                                    &requirement,
                                    &record.0,
                                    ctx.project_root,
                                ) {
                                    delayed_pypi_error.get_or_insert(err);
                                }

                                FoundPackage::PyPi(PypiPackageIdx(idx), requirement.extras.to_vec())
                            } else {
                                if let Err(err) = pypi_satifisfies_requirement(
                                    &requirement,
                                    &record.0,
                                    ctx.project_root,
                                ) {
                                    delayed_pypi_error.get_or_insert(err);
                                }

                                FoundPackage::PyPi(PypiPackageIdx(idx), requirement.extras.to_vec())
                            }
                        }
                        Ok(None) => {
                            // The record does not match the spec, the lock-file is inconsistent.
                            delayed_pypi_error.get_or_insert_with(|| {
                                Box::new(PlatformUnsat::UnsatisfiableRequirement(
                                    Box::new(requirement),
                                    source.into_owned(),
                                ))
                            });
                            continue;
                        }
                        Err(err) => {
                            // An error occurred while converting the package name.
                            delayed_pypi_error.get_or_insert_with(|| {
                                Box::new(PlatformUnsat::from(ConversionError::NameConversion(err)))
                            });
                            continue;
                        }
                    }
                }
            }
        };

        // Add all the requirements of the package to the queue.
        match found_package {
            FoundPackage::Conda(idx) => {
                if !conda_packages_visited.insert(idx) {
                    // We already visited this package, so we can skip adding its dependencies to
                    // the queue
                    continue;
                }

                let record = &locked_pixi_records.records[idx.0];
                for depends in &record.package_record().depends {
                    let spec = MatchSpec::from_str(depends.as_str(), Lenient)
                        .map_err(|e| PlatformUnsat::FailedToParseMatchSpec(depends.clone(), e))?;

                    let (origin, anchor) = match record {
                        PixiRecord::Binary(record) => (
                            Cow::Owned(record.file_name.to_string()),
                            SourceAnchor::Workspace,
                        ),
                        PixiRecord::Source(record) => (
                            Cow::Owned(format!(
                                "{} @ {}",
                                record.package_record.name.as_source(),
                                &record.manifest_source
                            )),
                            SourceSpec::from(record.manifest_source.clone()).into(),
                        ),
                    };

                    if let Some((source, package_name)) = record
                        .as_source()
                        .and_then(|record| Some((record, spec.name.as_ref()?)))
                        .and_then(|(record, package_name_matcher)| {
                            let package_name = package_name_matcher
                                .as_exact()
                                .expect("depends can only contain exact package names");
                            Some((
                                record.sources.get(package_name.as_normalized())?,
                                package_name,
                            ))
                        })
                    {
                        let anchored_location = anchor.resolve(source.location.clone());
                        let anchored_source = SourceSpec {
                            location: anchored_location,
                        };
                        conda_queue.push(Dependency::CondaSource(
                            package_name.clone(),
                            spec,
                            anchored_source,
                            origin,
                        ));
                    } else {
                        conda_queue.push(Dependency::Conda(spec, origin));
                    }
                }
            }
            FoundPackage::PyPi(idx, extras) => {
                let record = &locked_pypi_environment.records[idx.0];

                // If there is no marker environment there is no python version
                let Some(marker_environment) = marker_environment.as_ref() else {
                    return Err(Box::new(PlatformUnsat::MissingPythonInterpreter));
                };

                if pypi_packages_visited.insert(idx) {
                    // If this is path based package we need to check if the source tree hash still
                    // matches. and if it is a directory
                    if let UrlOrPath::Path(path) = &record.0.location {
                        let absolute_path = if path.is_absolute() {
                            Cow::Borrowed(Path::new(path.as_str()))
                        } else {
                            Cow::Owned(ctx.project_root.join(Path::new(path.as_str())))
                        };

                        if absolute_path.is_dir() {
                            // Read metadata using UV's DistributionDatabase.
                            // This first tries database.requires_dist() for static extraction,
                            // then falls back to building the wheel if needed.
                            let uv_ctx = ctx
                                .uv_context
                                .get_or_try_init(|| UvResolutionContext::from_config(ctx.config))
                                .map_err(|e| {
                                    Box::new(PlatformUnsat::FailedToReadLocalMetadata(
                                        record.0.name.clone(),
                                        format!("failed to initialize UV context: {e}"),
                                    ))
                                })?;

                            let mut build_ctx = BuildMetadataContext {
                                environment: ctx.environment,
                                locked_pixi_records,
                                platform: ctx.platform,
                                project_root: ctx.project_root,
                                uv_context: uv_ctx,
                                project_env_vars: &ctx.project_env_vars,
                                command_dispatcher: ctx.command_dispatcher.clone(),
                                build_caches: ctx.build_caches,
                                building_pixi_records: &building_pixi_records,
                                static_metadata_cache: ctx.static_metadata_cache,
                            };

                            match read_local_package_metadata(
                                &absolute_path,
                                &record.0.name,
                                record.0.editable,
                                &mut build_ctx,
                            )
                            .await
                            {
                                Ok(current_metadata) => {
                                    // Compare metadata with locked metadata
                                    if let Some(mismatch) = pypi_metadata::compare_metadata(
                                        &record.0,
                                        &current_metadata,
                                    ) {
                                        let local_mismatch = match mismatch {
                                            pypi_metadata::MetadataMismatch::RequiresDist(diff) => {
                                                LocalMetadataMismatch::RequiresDist {
                                                    added: diff.added,
                                                    removed: diff.removed,
                                                }
                                            }
                                            pypi_metadata::MetadataMismatch::Version {
                                                locked,
                                                current,
                                            } => LocalMetadataMismatch::Version { locked, current },
                                            pypi_metadata::MetadataMismatch::RequiresPython {
                                                locked,
                                                current,
                                            } => LocalMetadataMismatch::RequiresPython {
                                                locked,
                                                current,
                                            },
                                        };
                                        delayed_pypi_error.get_or_insert_with(|| {
                                            Box::new(PlatformUnsat::LocalPackageMetadataMismatch(
                                                record.0.name.clone(),
                                                local_mismatch,
                                            ))
                                        });
                                    }
                                }
                                Err(e) => {
                                    delayed_pypi_error.get_or_insert_with(|| {
                                        Box::new(PlatformUnsat::FailedToReadLocalMetadata(
                                            record.0.name.clone(),
                                            format!("failed to read metadata: {e}"),
                                        ))
                                    });
                                }
                            }
                        }
                    }

                    // Ensure that the record matches the currently selected interpreter.
                    if let Some(requires_python) = &record.0.requires_python {
                        let uv_specifier_requires_python = to_uv_specifiers(requires_python)
                            .expect("pep440 conversion should never fail");

                        let marker_version = pep440_rs::Version::from_str(
                            &marker_environment.python_full_version().version.to_string(),
                        )
                        .expect("cannot parse version");
                        let uv_maker_version = to_uv_version(&marker_version)
                            .expect("cannot convert python marker version to uv_pep440");

                        let marker_requires_python =
                            RequiresPython::greater_than_equal_version(&uv_maker_version);
                        // Use the function of RequiresPython object as it implements the lower
                        // bound logic Related issue https://github.com/astral-sh/uv/issues/4022
                        if !marker_requires_python.is_contained_by(&uv_specifier_requires_python) {
                            delayed_pypi_error.get_or_insert_with(|| {
                                Box::new(PlatformUnsat::PythonVersionMismatch(
                                    record.0.name.clone(),
                                    requires_python.clone(),
                                    marker_version.into(),
                                ))
                            });
                        }
                    }
                }

                // Add all the requirements of the package to the queue.
                for requirement in &record.0.requires_dist {
                    let requirement =
                        match pep508_requirement_to_uv_requirement(requirement.clone()) {
                            Ok(requirement) => requirement,
                            Err(err) => {
                                delayed_pypi_error.get_or_insert_with(|| {
                                    Box::new(ConversionError::NameConversion(err).into())
                                });
                                continue;
                            }
                        };

                    // Skip this requirement if it does not apply.
                    if !requirement.evaluate_markers(Some(marker_environment), &extras) {
                        continue;
                    }

                    // Skip this requirement if it has already been visited.
                    if !pypi_requirements_visited.insert(requirement.clone()) {
                        continue;
                    }

                    pypi_queue.push(Dependency::PyPi(
                        requirement.clone(),
                        record.0.name.as_ref().to_string().into(),
                    ));
                }
            }
        }
    }

    // Check if all locked packages have also been visited
    if conda_packages_visited.len() != locked_pixi_records.len() {
        return Err(Box::new(PlatformUnsat::TooManyCondaPackages(
            locked_pixi_records
                .names()
                .enumerate()
                .filter_map(|(idx, name)| {
                    if conda_packages_visited.contains(&CondaPackageIdx(idx)) {
                        None
                    } else {
                        Some(name.clone())
                    }
                })
                .collect(),
        )));
    }

    // Check if all records that are source records should actually be source
    // records. If there are no source specs in the environment for a particular
    // package than the package must be a binary package.
    for record in locked_pixi_records
        .records
        .iter()
        .filter_map(PixiRecord::as_source)
    {
        if !expected_conda_source_dependencies.contains(&record.package_record.name) {
            return Err(Box::new(PlatformUnsat::RequiredBinaryIsSource(
                record.package_record.name.as_source().to_string(),
            )));
        }
    }

    // Now that we checked all conda requirements, check if there were any pypi
    // issues.
    if let Some(err) = delayed_pypi_error {
        return Err(err);
    }

    if pypi_packages_visited.len() != locked_pypi_environment.len() {
        return Err(Box::new(PlatformUnsat::TooManyPypiPackages(
            locked_pypi_environment
                .names()
                .enumerate()
                .filter_map(|(idx, name)| {
                    if pypi_packages_visited.contains(&PypiPackageIdx(idx)) {
                        None
                    } else {
                        Some(name.clone())
                    }
                })
                .collect(),
        )));
    }

    // Note: Editability is NOT checked here. The lock file always stores
    // editable=false (which is omitted from serialization). Editability is
    // looked up from the manifest at install time. This allows different
    // environments in a solve-group to have different editability settings for
    // the same path-based package.

    // Verify the pixi build package's package_build_source matches the manifest.
    verify_build_source_matches_manifest(ctx.environment, locked_pixi_records)?;

    Ok(VerifiedIndividualEnvironment {
        expected_conda_packages,
        conda_packages_used_by_pypi,
    })
}

enum FoundPackage {
    Conda(CondaPackageIdx),
    PyPi(PypiPackageIdx, Vec<uv_normalize::ExtraName>),
}

/// An index into the list of conda packages.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
#[repr(transparent)]
pub struct CondaPackageIdx(usize);

/// An index into the list of pypi packages.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
#[repr(transparent)]
pub struct PypiPackageIdx(usize);

/// Context for building dynamic metadata for local packages.
struct BuildMetadataContext<'a> {
    environment: &'a Environment<'a>,
    locked_pixi_records: &'a PixiRecordsByName,
    platform: Platform,
    project_root: &'a Path,
    uv_context: &'a UvResolutionContext,
    project_env_vars: &'a HashMap<EnvironmentName, EnvironmentVars>,
    command_dispatcher: CommandDispatcher,
    build_caches: &'a mut HashMap<BuildCacheKey, Arc<EnvironmentBuildCache>>,
    building_pixi_records: &'a Result<PixiRecordsByName, PlatformUnsat>,
    static_metadata_cache: &'a mut HashMap<PathBuf, pypi_metadata::LocalPackageMetadata>,
}

/// Read metadata for a local directory package using UV's DistributionDatabase.
///
/// This first tries to extract metadata statically via `database.requires_dist()`,
/// which parses the pyproject.toml without building. If static extraction fails
/// (e.g., dynamic dependencies), it falls back to building the wheel metadata.
///
/// Static metadata is cached across platforms since it doesn't depend on the platform.
async fn read_local_package_metadata(
    directory: &Path,
    package_name: &pep508_rs::PackageName,
    editable: bool,
    ctx: &mut BuildMetadataContext<'_>,
) -> Result<pypi_metadata::LocalPackageMetadata, PlatformUnsat> {
    // Check if we already have static metadata cached for this directory
    if let Some(cached_metadata) = ctx.static_metadata_cache.get(directory) {
        tracing::debug!("Package {} - using cached static metadata", package_name);
        return Ok(cached_metadata.clone());
    }

    let pypi_options = ctx.environment.pypi_options();

    // Find the Python interpreter from locked records
    let python_record = ctx
        .locked_pixi_records
        .records
        .iter()
        .find(|r| is_python_record(r))
        .ok_or_else(|| {
            PlatformUnsat::FailedToReadLocalMetadata(
                package_name.clone(),
                "No Python interpreter found in locked packages".to_string(),
            )
        })?;

    // Create marker environment for the target platform
    let marker_environment = determine_marker_environment(ctx.platform, python_record.as_ref())
        .map_err(|e| {
            PlatformUnsat::FailedToReadLocalMetadata(
                package_name.clone(),
                format!("Failed to determine marker environment: {e}"),
            )
        })?;

    let index_strategy = to_index_strategy(pypi_options.index_strategy.as_ref());

    // Get or create cache entry for this environment and host platform
    // We use best_platform() since the build prefix is shared across all target platforms
    let best_platform = ctx.environment.best_platform();
    let cache_key = BuildCacheKey::new(ctx.environment.name().clone(), best_platform);
    let cache = ctx.build_caches.entry(cache_key).or_default();

    let index_locations = pypi_options_to_index_locations(&pypi_options, ctx.project_root)
        .map_err(|e| {
            PlatformUnsat::FailedToReadLocalMetadata(
                package_name.clone(),
                format!("Failed to setup index locations: {e}"),
            )
        })?;

    let build_options = pypi_options_to_build_options(
        &pypi_options.no_build.clone().unwrap_or_default(),
        &pypi_options.no_binary.clone().unwrap_or_default(),
    )
    .map_err(|e| {
        PlatformUnsat::FailedToReadLocalMetadata(
            package_name.clone(),
            format!("Failed to create build options: {e}"),
        )
    })?;

    let dependency_metadata = DependencyMetadata::default();

    // Configure insecure hosts
    let allow_insecure_hosts = configure_insecure_hosts_for_tls_bypass(
        ctx.uv_context.allow_insecure_host.clone(),
        ctx.uv_context.tls_no_verify,
        &index_locations,
    );

    let registry_client = {
        let base_client_builder = BaseClientBuilder::default()
            .allow_insecure_host(allow_insecure_hosts.clone())
            .markers(&marker_environment)
            .keyring(ctx.uv_context.keyring_provider)
            .connectivity(Connectivity::Online)
            .native_tls(ctx.uv_context.use_native_tls)
            .extra_middleware(ctx.uv_context.extra_middleware.clone());

        let mut uv_client_builder =
            RegistryClientBuilder::new(base_client_builder, ctx.uv_context.cache.clone())
                .index_locations(index_locations.clone())
                .index_strategy(index_strategy);

        for p in &ctx.uv_context.proxies {
            uv_client_builder = uv_client_builder.proxy(p.clone())
        }

        Arc::new(uv_client_builder.build())
    };

    // Get tags for this platform (needed for FlatIndex)
    let system_requirements = ctx.environment.system_requirements();
    let tags =
        get_pypi_tags(ctx.platform, &system_requirements, python_record.as_ref()).map_err(|e| {
            PlatformUnsat::FailedToReadLocalMetadata(
                package_name.clone(),
                format!("Failed to determine pypi tags: {e}"),
            )
        })?;

    let flat_index = {
        let flat_index_client = FlatIndexClient::new(
            registry_client.cached_client(),
            Connectivity::Online,
            &ctx.uv_context.cache,
        );
        let flat_index_urls: Vec<&IndexUrl> = index_locations
            .flat_indexes()
            .map(|index| index.url())
            .collect();
        let flat_index_entries = flat_index_client
            .fetch_all(flat_index_urls.into_iter())
            .await
            .map_err(|e| {
                PlatformUnsat::FailedToReadLocalMetadata(
                    package_name.clone(),
                    format!("Failed to fetch flat index entries: {e}"),
                )
            })?;
        FlatIndex::from_entries(
            flat_index_entries,
            Some(&tags),
            &ctx.uv_context.hash_strategy,
            &build_options,
        )
    };

    // Create build dispatch parameters
    let config_settings = ConfigSettings::default();
    let build_params = UvBuildDispatchParams::new(
        &registry_client,
        &ctx.uv_context.cache,
        &index_locations,
        &flat_index,
        &dependency_metadata,
        &config_settings,
        &build_options,
        &ctx.uv_context.hash_strategy,
    )
    .with_index_strategy(index_strategy)
    .with_workspace_cache(ctx.uv_context.workspace_cache.clone())
    .with_shared_state(ctx.uv_context.shared_state.fork())
    .with_source_strategy(ctx.uv_context.source_strategy)
    .with_concurrency(ctx.uv_context.concurrency);

    // Get or create conda prefix updater for the environment
    // Use best_platform() because we can only install/run Python on the host platform
    let conda_prefix_updater = cache
        .conda_prefix_updater
        .get_or_try_init(|| {
            let prefix_platform = ctx.environment.best_platform();
            let group = GroupedEnvironment::Environment(ctx.environment.clone());
            let virtual_packages = ctx.environment.virtual_packages(prefix_platform);

            // Force the initialization of the rayon thread pool to avoid implicit creation
            // by the uv.
            LazyLock::force(&RAYON_INITIALIZE);

            CondaPrefixUpdater::builder(
                group,
                prefix_platform,
                virtual_packages
                    .into_iter()
                    .map(GenericVirtualPackage::from)
                    .collect(),
                ctx.command_dispatcher.clone(),
            )
            .finish()
            .map_err(|e| {
                PlatformUnsat::FailedToReadLocalMetadata(
                    package_name.clone(),
                    format!("Failed to create conda prefix updater: {e}"),
                )
            })
        })?
        .clone();

    // Use cached lazy build dispatch dependencies
    // Use building_pixi_records (host platform) for installing Python and building,
    // since we can only run binaries on the host platform
    let building_records: miette::Result<Vec<PixiRecord>> = ctx
        .building_pixi_records
        .as_ref()
        .map(|r| r.records.clone())
        .map_err(|e| miette::miette!("{}", e));
    let lazy_build_dispatch = LazyBuildDispatch::new(
        build_params,
        conda_prefix_updater,
        ctx.project_env_vars.clone(),
        ctx.environment.clone(),
        building_records,
        pypi_options.no_build_isolation.clone(),
        &cache.lazy_build_dispatch_deps,
        None,
        false,
    );

    // Create distribution database
    let database = DistributionDatabase::new(
        &registry_client,
        &lazy_build_dispatch,
        ctx.uv_context.concurrency.downloads,
    );

    // Try to read pyproject.toml and use requires_dist() first
    let pyproject_path = directory.join("pyproject.toml");
    if let Ok(contents) = fs_err::read_to_string(&pyproject_path) {
        // Parse with toml_edit for version/requires_python
        if let Ok(toml) = contents.parse::<toml_edit::DocumentMut>() {
            // Extract version and requires_python
            let version_is_dynamic = toml
                .get("project")
                .and_then(|p| p.get("dynamic"))
                .and_then(|d| d.as_array())
                .is_some_and(|arr| arr.iter().any(|item| item.as_str() == Some("version")));

            let version = if version_is_dynamic {
                None
            } else {
                toml.get("project")
                    .and_then(|p| p.get("version"))
                    .and_then(|v| v.as_str())
                    .and_then(|v| v.parse::<pep440_rs::Version>().ok())
            };

            let requires_python = toml
                .get("project")
                .and_then(|p| p.get("requires-python"))
                .and_then(|v| v.as_str())
                .and_then(|rp| rp.parse::<VersionSpecifiers>().ok());

            // Parse pyproject.toml with UV's parser for requires_dist
            if let Ok(pyproject_toml) = PyProjectToml::from_toml(&contents) {
                // Try to extract requires_dist statically using UV's database
                match database.requires_dist(directory, &pyproject_toml).await {
                    Ok(Some(requires_dist)) if !requires_dist.dynamic => {
                        tracing::debug!(
                            "Package {} - extracted requires_dist using database.requires_dist(). Dynamic: {}",
                            package_name,
                            requires_dist.dynamic
                        );

                        // Convert uv requirements to pep508_rs requirements
                        let requires_dist_converted: Result<Vec<pep508_rs::Requirement>, _> =
                            requires_dist
                                .requires_dist
                                .iter()
                                .map(|req| {
                                    let req_str = req.to_string();
                                    req_str.parse::<pep508_rs::Requirement>().map_err(|e| {
                                        PlatformUnsat::FailedToReadLocalMetadata(
                                            package_name.clone(),
                                            format!("Invalid requirement: {e}"),
                                        )
                                    })
                                })
                                .collect();

                        if let Ok(requires_dist_vec) = requires_dist_converted {
                            let metadata = pypi_metadata::LocalPackageMetadata {
                                version,
                                requires_dist: requires_dist_vec,
                                requires_python,
                                is_version_dynamic: requires_dist.dynamic,
                            };
                            // Cache the static metadata for reuse on other platforms
                            ctx.static_metadata_cache
                                .insert(directory.to_path_buf(), metadata.clone());
                            return Ok(metadata);
                        }
                    }
                    Ok(Some(requires_dist)) => {
                        // Dynamic dependencies - need to build wheel for accurate metadata
                        tracing::debug!(
                            "Package {} - requires_dist is dynamic (dynamic={}), falling back to wheel build",
                            package_name,
                            requires_dist.dynamic
                        );
                    }
                    Ok(None) => {
                        tracing::debug!(
                            "Package {} - requires_dist() returned None, falling back to build",
                            package_name
                        );
                    }
                    Err(e) => {
                        tracing::debug!(
                            "Package {} - requires_dist() failed: {}, falling back to build",
                            package_name,
                            e
                        );
                    }
                }
            }
        }
    }

    // Fall back to building the wheel metadata
    tracing::debug!(
        "Package {} - building wheel metadata with get_or_build_wheel_metadata()",
        package_name
    );

    // Create the directory source dist
    let uv_package_name =
        uv_normalize::PackageName::from_str(package_name.as_ref()).map_err(|e| {
            PlatformUnsat::FailedToReadLocalMetadata(
                package_name.clone(),
                format!("Invalid package name: {e}"),
            )
        })?;

    let install_path = directory.to_path_buf();
    let file_url = url::Url::from_file_path(&install_path).map_err(|_| {
        PlatformUnsat::FailedToReadLocalMetadata(
            package_name.clone(),
            format!("Failed to convert path to URL: {}", install_path.display()),
        )
    })?;
    let verbatim_url = uv_pep508::VerbatimUrl::from_url(file_url.into());
    let source_dist = DirectorySourceDist {
        name: uv_package_name,
        install_path: install_path.into_boxed_path(),
        editable: Some(editable),
        r#virtual: Some(false),
        url: verbatim_url,
    };

    // Build the metadata
    let metadata_response = database
        .get_or_build_wheel_metadata(
            &Dist::Source(SourceDist::Directory(source_dist)),
            HashPolicy::None,
        )
        .await
        .map_err(|e| {
            PlatformUnsat::FailedToReadLocalMetadata(
                package_name.clone(),
                format!("Failed to build metadata: {e}"),
            )
        })?;

    // Convert UV metadata to our format
    pypi_metadata::from_uv_metadata(&metadata_response.metadata).map_err(|e| {
        PlatformUnsat::FailedToReadLocalMetadata(
            package_name.clone(),
            format!("Failed to convert metadata: {e}"),
        )
    })
}

fn find_matching_package(
    locked_pixi_records: &PixiRecordsByName,
    virtual_packages: &HashMap<PackageName, GenericVirtualPackage>,
    spec: MatchSpec,
    source: Cow<str>,
) -> Result<Option<CondaPackageIdx>, Box<PlatformUnsat>> {
    let found_package = match &spec.name {
        None => {
            // No name means we have to find any package that matches the spec.
            match locked_pixi_records
                .records
                .iter()
                .position(|record| spec.matches(record))
            {
                None => {
                    // No records match the spec.
                    return Err(Box::new(PlatformUnsat::UnsatisfiableMatchSpec(
                        Box::new(spec),
                        source.into_owned(),
                    )));
                }
                Some(idx) => idx,
            }
        }
        Some(name_matcher) => {
            let name = name_matcher
                .as_exact()
                .expect("depends can only contain exact package names");
            match locked_pixi_records
                .index_by_name(name)
                .map(|idx| (idx, &locked_pixi_records.records[idx]))
            {
                Some((idx, record)) if spec.matches(record) => idx,
                Some(_) => {
                    // The record does not match the spec, the lock-file is
                    // inconsistent.
                    return Err(Box::new(PlatformUnsat::UnsatisfiableMatchSpec(
                        Box::new(spec),
                        source.into_owned(),
                    )));
                }
                None => {
                    // Check if there is a virtual package by that name
                    if let Some(vpkg) = virtual_packages.get(name.as_normalized()) {
                        if vpkg.matches(&spec) {
                            // The matchspec matches a virtual package. No need to
                            // propagate the dependencies.
                            return Ok(None);
                        } else {
                            // The record does not match the spec, the lock-file is
                            // inconsistent.
                            return Err(Box::new(PlatformUnsat::UnsatisfiableMatchSpec(
                                Box::new(spec),
                                source.into_owned(),
                            )));
                        }
                    } else {
                        // The record does not match the spec, the lock-file is
                        // inconsistent.
                        return Err(Box::new(PlatformUnsat::UnsatisfiableMatchSpec(
                            Box::new(spec),
                            source.into_owned(),
                        )));
                    }
                }
            }
        }
    };

    Ok(Some(CondaPackageIdx(found_package)))
}

fn find_matching_source_package(
    locked_pixi_records: &PixiRecordsByName,
    name: PackageName,
    source_spec: SourceSpec,
    source: Cow<str>,
    match_spec: Option<MatchSpec>,
) -> Result<CondaPackageIdx, Box<PlatformUnsat>> {
    // Find the package that matches the source spec.
    let Some((idx, package)) = locked_pixi_records
        .index_by_name(&name)
        .map(|idx| (idx, &locked_pixi_records.records[idx]))
    else {
        // The record does not match the spec, the lock-file is
        // inconsistent.
        return Err(Box::new(PlatformUnsat::SourcePackageMissing(
            name.as_source().to_string(),
            source.into_owned(),
        )));
    };

    let PixiRecord::Source(source_package) = package else {
        return Err(Box::new(PlatformUnsat::RequiredSourceIsBinary(
            name.as_source().to_string(),
            source.into_owned(),
        )));
    };

    source_package
        .manifest_source
        .satisfies(&source_spec)
        .map_err(|e| PlatformUnsat::SourcePackageMismatch(name.as_source().to_string(), e))?;

    if let Some(match_spec) = match_spec
        && !match_spec.matches(package)
    {
        return Err(Box::new(PlatformUnsat::UnsatisfiableMatchSpec(
            Box::new(match_spec),
            source.into_owned(),
        )));
    }

    Ok(CondaPackageIdx(idx))
}

trait MatchesMatchspec {
    fn matches(&self, spec: &MatchSpec) -> bool;
}

impl MatchesMatchspec for GenericVirtualPackage {
    fn matches(&self, spec: &MatchSpec) -> bool {
        if let Some(name) = &spec.name
            && !name.matches(&self.name)
        {
            return false;
        }

        if let Some(version) = &spec.version
            && !version.matches(&self.version)
        {
            return false;
        }

        if let Some(build) = &spec.build
            && !build.matches(&self.build_string)
        {
            return false;
        }

        true
    }
}

pub fn verify_solve_group_satisfiability(
    environments: impl IntoIterator<Item = VerifiedIndividualEnvironment>,
) -> Result<(), SolveGroupUnsat> {
    let mut expected_conda_packages = HashSet::new();
    let mut conda_packages_used_by_pypi = HashSet::new();

    // Group all conda requested packages and pypi requested packages
    for env in environments {
        expected_conda_packages.extend(env.expected_conda_packages.into_iter());
        conda_packages_used_by_pypi.extend(env.conda_packages_used_by_pypi.into_iter());
    }

    // Check if all conda packages are also requested by another conda package.
    if let Some(conda_package) = conda_packages_used_by_pypi
        .into_iter()
        .find(|pkg| !expected_conda_packages.contains(pkg))
    {
        return Err(SolveGroupUnsat::CondaPackageShouldBePypi {
            name: conda_package.as_source().to_string(),
        });
    }

    Ok(())
}

/// Verify that the current package's build.source in the manifest
/// matches the lock file's `package_build_source` (if applicable).
/// Path-based sources are not represented in the lock file's
/// `package_build_source` and are skipped.
fn verify_build_source_matches_manifest(
    environment: &Environment<'_>,
    locked_pixi_records: &PixiRecordsByName,
) -> Result<(), Box<PlatformUnsat>> {
    let Some(pkg_manifest) = environment.workspace().package.as_ref() else {
        return Ok(());
    };
    let Some(pkg_name) = &pkg_manifest.value.package.name else {
        return Ok(());
    };
    let package_name = PackageName::new_unchecked(pkg_name);
    let manifest_source_location = pkg_manifest.value.build.source.clone();

    // Find the source record for the current package in locked conda packages.
    let Some(record) = locked_pixi_records.by_name(&package_name) else {
        return Ok(());
    };

    let PixiRecord::Source(src_record) = record else {
        return Ok(());
    };

    let lockfile_source_location = src_record.build_source.clone();

    let ok = Ok(());
    let error = Err(Box::new(PlatformUnsat::PackageBuildSourceMismatch(
        src_record.package_record.name.as_source().to_string(),
        SourceMismatchError::SourceTypeMismatch,
    )));
    let sat_err = |e| {
        Box::new(PlatformUnsat::PackageBuildSourceMismatch(
            src_record.package_record.name.as_source().to_string(),
            e,
        ))
    };

    match (manifest_source_location, lockfile_source_location) {
        (None, None) => ok,
        (Some(SourceLocationSpec::Url(murl_spec)), Some(PinnedSourceSpec::Url(lurl_spec))) => {
            lurl_spec.satisfies(&murl_spec).map_err(sat_err)
        }
        (
            Some(SourceLocationSpec::Git(mut mgit_spec)),
            Some(PinnedSourceSpec::Git(mut lgit_spec)),
        ) => {
            // Ignore subdirectory for comparison, they should not
            // trigger lockfile invalidation.
            mgit_spec.subdirectory = None;
            lgit_spec.source.subdirectory = None;

            // Ensure that we always compare references.
            if mgit_spec.rev.is_none() {
                mgit_spec.rev = Some(pixi_spec::GitReference::DefaultBranch);
            }
            lgit_spec.satisfies(&mgit_spec).map_err(sat_err)
        }
        (Some(SourceLocationSpec::Path(mpath_spec)), Some(PinnedSourceSpec::Path(lpath_spec))) => {
            lpath_spec.satisfies(&mpath_spec).map_err(sat_err)
        }
        // If they not equal kind we error-out
        (_, _) => error,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsStr,
        path::{Component, PathBuf},
        str::FromStr,
    };

    use insta::Settings;
    use miette::{IntoDiagnostic, NarratableReportHandler};
    use pep440_rs::{Operator, Version};
    use pixi_build_backend_passthrough::PassthroughBackend;
    use pixi_build_frontend::BackendOverride;
    use pixi_command_dispatcher::CacheDirs;
    use rattler_lock::LockFile;
    use rstest::rstest;
    use tracing_test::traced_test;

    use super::*;
    use crate::Workspace;

    #[derive(Error, Debug, Diagnostic)]
    enum LockfileUnsat {
        #[error("environment '{0}' is missing")]
        EnvironmentMissing(String),

        #[error("environment '{0}' does not satisfy the requirements of the project")]
        Environment(String, #[source] EnvironmentUnsat),

        #[error(
            "environment '{0}' does not satisfy the requirements of the project for platform '{1}'"
        )]
        PlatformUnsat(String, Platform, #[source] PlatformUnsat),

        #[error(
            "solve group '{0}' does not satisfy the requirements of the project for platform '{1}'"
        )]
        SolveGroupUnsat(String, Platform, #[source] SolveGroupUnsat),
    }

    async fn verify_lockfile_satisfiability(
        project: &Workspace,
        lock_file: &LockFile,
        backend_override: Option<BackendOverride>,
    ) -> Result<(), LockfileUnsat> {
        let mut individual_verified_envs = HashMap::new();

        let temp_pixi_dir = tempfile::tempdir().unwrap();
        let command_dispatcher = {
            let command_dispatcher = project
                .command_dispatcher_builder()
                .unwrap()
                .with_cache_dirs(CacheDirs::new(
                    pixi_path::AbsPathBuf::new(temp_pixi_dir.path())
                        .expect("tempdir path should be absolute")
                        .into_assume_dir(),
                ));
            let command_dispatcher = if let Some(backend_override) = backend_override {
                command_dispatcher.with_backend_overrides(backend_override)
            } else {
                command_dispatcher
            };
            command_dispatcher.finish()
        };

        // Create UV context lazily for building dynamic metadata
        let uv_context: OnceCell<UvResolutionContext> = OnceCell::new();

        // Create build caches for sharing between satisfiability and resolution
        let mut build_caches: HashMap<BuildCacheKey, Arc<EnvironmentBuildCache>> = HashMap::new();

        // Create static metadata cache for sharing across platforms
        let mut static_metadata_cache: HashMap<PathBuf, pypi_metadata::LocalPackageMetadata> =
            HashMap::new();

        // Verify individual environment satisfiability
        for env in project.environments() {
            let locked_env = lock_file
                .environment(env.name().as_str())
                .ok_or_else(|| LockfileUnsat::EnvironmentMissing(env.name().to_string()))?;
            verify_environment_satisfiability(&env, locked_env)
                .map_err(|e| LockfileUnsat::Environment(env.name().to_string(), e))?;

            for platform in env.platforms() {
                let mut ctx = VerifySatisfiabilityContext {
                    environment: &env,
                    command_dispatcher: command_dispatcher.clone(),
                    platform,
                    project_root: project.root(),
                    uv_context: &uv_context,
                    config: project.config(),
                    project_env_vars: project.env_vars().clone(),
                    build_caches: &mut build_caches,
                    static_metadata_cache: &mut static_metadata_cache,
                };
                let verified_env = verify_platform_satisfiability(&mut ctx, locked_env)
                    .await
                    .map_err(|e| {
                        LockfileUnsat::PlatformUnsat(env.name().to_string(), platform, *e)
                    })?;

                individual_verified_envs.insert((env.name(), platform), verified_env);
            }
        }

        // Verify the solve group requirements
        for solve_group in project.solve_groups() {
            for platform in solve_group.platforms() {
                verify_solve_group_satisfiability(
                    solve_group
                        .environments()
                        .filter_map(|env| individual_verified_envs.remove(&(env.name(), platform))),
                )
                .map_err(|e| {
                    LockfileUnsat::SolveGroupUnsat(solve_group.name().to_string(), platform, e)
                })?;
            }
        }

        // Verify environments not part of a solve group
        for ((env_name, platform), verified_env) in individual_verified_envs.into_iter() {
            verify_solve_group_satisfiability([verified_env])
                .map_err(|e| match e {
                    SolveGroupUnsat::CondaPackageShouldBePypi { name } => {
                        PlatformUnsat::CondaPackageShouldBePypi { name }
                    }
                })
                .map_err(|e| LockfileUnsat::PlatformUnsat(env_name.to_string(), platform, e))?;
        }

        Ok(())
    }

    #[rstest]
    #[tokio::test]
    #[traced_test]
    async fn test_good_satisfiability(
        #[files("../../tests/data/satisfiability/*/pixi.toml")] manifest_path: PathBuf,
    ) {
        // TODO: skip this test on windows
        // Until we can figure out how to handle unix file paths with pep508_rs url
        // parsing correctly
        if manifest_path
            .components()
            .contains(&Component::Normal(OsStr::new("absolute-paths")))
            && cfg!(windows)
        {
            return;
        }

        let project = Workspace::from_path(&manifest_path).unwrap();
        let lock_file = LockFile::from_path(&project.lock_file_path()).unwrap();
        match verify_lockfile_satisfiability(
            &project,
            &lock_file,
            Some(BackendOverride::from_memory(
                PassthroughBackend::instantiator(),
            )),
        )
        .await
        .into_diagnostic()
        {
            Ok(()) => {}
            Err(e) => panic!("{e:?}"),
        }
    }

    #[rstest]
    #[tokio::test]
    #[traced_test]
    async fn test_example_satisfiability(
        #[files("../../examples/**/p*.toml")] manifest_path: PathBuf,
    ) {
        // If a pyproject.toml is present check for `tool.pixi` in the file to avoid
        // testing of non-pixi files
        if manifest_path.file_name().unwrap() == "pyproject.toml" {
            let manifest_str = fs_err::read_to_string(&manifest_path).unwrap();
            if !manifest_str.contains("tool.pixi.workspace") {
                return;
            }
        }

        // If a pixi.toml is present check for `workspace` in the file to avoid
        // testing of non-pixi workspace files
        if manifest_path.file_name().unwrap() == "pixi.toml" {
            let manifest_str = fs_err::read_to_string(&manifest_path).unwrap();
            if !manifest_str.contains("workspace") {
                return;
            }
        }

        let project = Workspace::from_path(&manifest_path).unwrap();
        let lock_file = LockFile::from_path(&project.lock_file_path()).unwrap();
        match verify_lockfile_satisfiability(&project, &lock_file, None)
            .await
            .into_diagnostic()
        {
            Ok(()) => {}
            Err(e) => panic!("{e:?}"),
        }
    }

    #[rstest]
    #[tokio::test]
    #[traced_test]
    async fn test_failing_satisiability(
        #[files("../../tests/data/non-satisfiability/*/pixi.toml")] manifest_path: PathBuf,
    ) {
        let report_handler = NarratableReportHandler::new().with_cause_chain();

        let project = Workspace::from_path(&manifest_path).unwrap();
        let lock_file = LockFile::from_path(&project.lock_file_path()).unwrap();
        let err = verify_lockfile_satisfiability(
            &project,
            &lock_file,
            Some(BackendOverride::from_memory(
                PassthroughBackend::instantiator(),
            )),
        )
        .await
        .expect_err("expected failing satisfiability");

        let name = manifest_path
            .parent()
            .unwrap()
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap();

        let mut s = String::new();
        report_handler.render_report(&mut s, &err).unwrap();

        let mut settings = Settings::clone_current();
        settings.set_snapshot_suffix(name);
        settings.bind(|| {
            // run snapshot test here
            insta::assert_snapshot!(s);
        });
    }

    #[test]
    fn test_pypi_git_check_with_rev() {
        // Mock locked data
        let locked_data = PypiPackageData {
            name: "mypkg".parse().unwrap(),
            version: Version::from_str("0.1.0").unwrap(),
            location: "git+https://github.com/mypkg@rev=29932f3915935d773dc8d52c292cadd81c81071d#29932f3915935d773dc8d52c292cadd81c81071d"
                .parse()
                .expect("failed to parse url"),
            hash: None,
            requires_dist: vec![],
            requires_python: None,
            editable: false,
        };
        let spec = pep508_requirement_to_uv_requirement(
            pep508_rs::Requirement::from_str("mypkg @ git+https://github.com/mypkg@2993").unwrap(),
        )
        .unwrap();
        let project_root = PathBuf::from_str("/").unwrap();
        // This will not satisfy because the rev length is different, even being
        // resolved to the same one
        pypi_satifisfies_requirement(&spec, &locked_data, &project_root).unwrap_err();

        let locked_data = PypiPackageData {
            name: "mypkg".parse().unwrap(),
            version: Version::from_str("0.1.0").unwrap(),
            location: "git+https://github.com/mypkg.git?rev=29932f3915935d773dc8d52c292cadd81c81071d#29932f3915935d773dc8d52c292cadd81c81071d"
                .parse()
                .expect("failed to parse url"),
            hash: None,
            requires_dist: vec![],
            requires_python: None,
            editable: false,
        };
        let spec = pep508_requirement_to_uv_requirement(
            pep508_rs::Requirement::from_str(
                "mypkg @ git+https://github.com/mypkg.git@29932f3915935d773dc8d52c292cadd81c81071d",
            )
            .unwrap(),
        )
        .unwrap();
        let project_root = PathBuf::from_str("/").unwrap();
        // This will satisfy
        pypi_satifisfies_requirement(&spec, &locked_data, &project_root).unwrap();
        let non_matching_spec = pep508_requirement_to_uv_requirement(
            pep508_rs::Requirement::from_str("mypkg @ git+https://github.com/mypkg@defgd").unwrap(),
        )
        .unwrap();
        // This should not
        pypi_satifisfies_requirement(&non_matching_spec, &locked_data, &project_root).unwrap_err();
        // Removing the rev from the Requirement should satisfy any revision
        let spec = pep508_requirement_to_uv_requirement(
            pep508_rs::Requirement::from_str("mypkg @ git+https://github.com/mypkg").unwrap(),
        )
        .unwrap();
        pypi_satifisfies_requirement(&spec, &locked_data, &project_root).unwrap();
    }

    // Currently this test is missing from `good_satisfiability`, so we test the
    // specific windows case here this should work an all supported platforms
    #[test]
    fn test_windows_absolute_path_handling() {
        // Mock locked data
        let locked_data = PypiPackageData {
            name: "mypkg".parse().unwrap(),
            version: Version::from_str("0.1.0").unwrap(),
            location: UrlOrPath::Path("C:\\Users\\username\\mypkg.tar.gz".into()),
            hash: None,
            requires_dist: vec![],
            requires_python: None,
            editable: false,
        };

        let spec =
            pep508_rs::Requirement::from_str("mypkg @ file:///C:\\Users\\username\\mypkg.tar.gz")
                .unwrap();

        let spec = pep508_requirement_to_uv_requirement(spec).unwrap();

        // This should satisfy:
        pypi_satifisfies_requirement(&spec, &locked_data, Path::new("")).unwrap();
    }

    // Validate uv documentation to avoid breaking change in pixi
    #[test]
    fn test_version_specifiers_logic() {
        let version = Version::from_str("1.19").unwrap();
        let version_specifiers = VersionSpecifiers::from_str("<2.0, >=1.16").unwrap();
        assert!(version_specifiers.contains(&version));
        // VersionSpecifiers derefs into a list of specifiers
        assert_eq!(
            version_specifiers
                .iter()
                .position(|specifier| *specifier.operator() == Operator::LessThan),
            Some(1)
        );
    }
}
