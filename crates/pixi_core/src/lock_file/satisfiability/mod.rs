use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fmt::{Display, Formatter},
    hash::Hash,
    ops::Sub,
    path::{Path, PathBuf},
    str::FromStr,
};

use itertools::{Either, Itertools};
use miette::Diagnostic;
use pep440_rs::VersionSpecifiers;
use pixi_build_discovery::{DiscoveredBackend, EnabledProtocols};
use pixi_build_type_conversions::compute_project_model_hash;
use pixi_git::url::RepositoryUrl;
use pixi_glob::{GlobHashCache, GlobHashError, GlobHashKey};
use pixi_manifest::{FeaturesExt, pypi::pypi_options::NoBuild};
use pixi_record::{LockedGitUrl, ParseLockFileError, PixiRecord, SourceMismatchError};
use pixi_spec::{PixiSpec, SourceAnchor, SourceSpec, SpecConversionError};
use pixi_uv_conversions::{
    AsPep508Error, as_uv_req, into_pixi_reference, pep508_requirement_to_uv_requirement,
    to_normalize, to_uv_specifiers, to_uv_version,
};
use pypi_modifiers::pypi_marker_env::determine_marker_environment;
use rattler_conda_types::{
    ChannelUrl, GenericVirtualPackage, MatchSpec, Matches, NamedChannelOrUrl, ParseChannelError,
    ParseMatchSpecError, ParseStrictness::Lenient, Platform,
};
use rattler_lock::{
    LockedPackageRef, PackageHashes, PypiIndexes, PypiPackageData, PypiSourceTreeHashable,
    UrlOrPath,
};
use thiserror::Error;
use typed_path::Utf8TypedPathBuf;
use url::Url;
use uv_distribution_filename::{DistExtension, ExtensionError, SourceDistExtension};
use uv_distribution_types::RequirementSource;
use uv_distribution_types::RequiresPython;
use uv_git_types::GitReference;
use uv_pypi_types::ParsedUrlError;

use super::{
    PixiRecordsByName, PypiRecord, PypiRecordsByName, package_identifier::ConversionError,
};
use crate::workspace::{Environment, grouped_environment::GroupedEnvironment};

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
pub struct EditablePackagesMismatch {
    pub expected_editable: Vec<uv_normalize::PackageName>,
    pub unexpected_editable: Vec<uv_normalize::PackageName>,
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
            .map(|hash| format!("{:x}", hash))
            .or(self.computed.md5().map(|hash| format!("{:x}", hash)));
        let locked_hash = self.locked.as_ref().and_then(|hash| {
            hash.sha256()
                .map(|hash| format!("{:x}", hash))
                .or(hash.md5().map(|hash| format!("{:x}", hash)))
        });

        match (computed_hash, locked_hash) {
            (None, None) => write!(f, "could not compute a source tree hash"),
            (Some(computed), None) => {
                write!(
                    f,
                    "the computed source tree hash is '{}', but the lock-file does not contain a hash",
                    computed
                )
            }
            (Some(computed), Some(locked)) => write!(
                f,
                "the computed source tree hash is '{}', but the lock-file contains '{}'",
                computed, locked
            ),
            (None, Some(locked)) => write!(
                f,
                "could not compute a source tree hash, but the lock-file contains '{}'",
                locked
            ),
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

    #[error("there are more conda packages in the lock-file than are used by the environment: {}", .0.iter().map(rattler_conda_types::PackageName::as_source).format(", "))]
    TooManyCondaPackages(Vec<rattler_conda_types::PackageName>),

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

    #[error(transparent)]
    EditablePackageMismatch(EditablePackagesMismatch),

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

    #[error("the path '{0}, cannot be canonicalized")]
    FailedToCanonicalizePath(PathBuf, #[source] std::io::Error),

    #[error(transparent)]
    FailedToComputeInputHash(#[from] GlobHashError),

    #[error("the input hash for '{0}' ({1}) does not match the hash in the lock-file ({2})")]
    InputHashMismatch(String, String, String),

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

    #[error("'{name}' is locked as a conda package but only requested by pypi dependencies")]
    CondaPackageShouldBePypi { name: String },
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
                | PlatformUnsat::EditablePackageMismatch(_)
                | PlatformUnsat::SourceTreeHashMismatch(..),
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
    if locked_environment.solve_options().strategy != environment.solve_strategy() {
        return Err(EnvironmentUnsat::SolveStrategyMismatch {
            locked_strategy: locked_environment.solve_options().strategy,
            expected_strategy: environment.solve_strategy(),
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
pub async fn verify_platform_satisfiability(
    environment: &Environment<'_>,
    locked_environment: rattler_lock::Environment<'_>,
    platform: Platform,
    project_root: &Path,
    glob_hash_cache: GlobHashCache,
) -> Result<VerifiedIndividualEnvironment, Box<PlatformUnsat>> {
    // Convert the lock file into a list of conda and pypi packages
    let mut pixi_records: Vec<PixiRecord> = Vec::new();
    let mut pypi_packages: Vec<PypiRecord> = Vec::new();
    for package in locked_environment.packages(platform).into_iter().flatten() {
        match package {
            LockedPackageRef::Conda(conda) => {
                let url = conda.location().clone();
                pixi_records.push(
                    conda
                        .clone()
                        .try_into()
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
    if environment.has_pypi_dependencies()
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
    let pixi_records_by_name = match PixiRecordsByName::from_unique_iter(pixi_records) {
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

    verify_package_platform_satisfiability(
        environment,
        &pixi_records_by_name,
        &pypi_records_by_name,
        platform,
        project_root,
        glob_hash_cache,
    )
    .await
}

#[allow(clippy::large_enum_variant)]
enum Dependency {
    Input(
        rattler_conda_types::PackageName,
        PixiSpec,
        Cow<'static, str>,
    ),
    Conda(MatchSpec, Cow<'static, str>),
    CondaSource(
        rattler_conda_types::PackageName,
        MatchSpec,
        SourceSpec,
        Cow<'static, str>,
    ),
    PyPi(uv_distribution_types::Requirement, Cow<'static, str>),
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
    pub expected_conda_packages: HashSet<rattler_conda_types::PackageName>,

    /// All conda packages that satisfy a pypi requirement.
    pub conda_packages_used_by_pypi: HashSet<rattler_conda_types::PackageName>,
}

pub(crate) async fn verify_package_platform_satisfiability(
    environment: &Environment<'_>,
    locked_pixi_records: &PixiRecordsByName,
    locked_pypi_environment: &PypiRecordsByName,
    platform: Platform,
    project_root: &Path,
    input_hash_cache: GlobHashCache,
) -> Result<VerifiedIndividualEnvironment, Box<PlatformUnsat>> {
    let channel_config = environment.channel_config();

    // Determine the dependencies requested by the environment
    let environment_dependencies = environment
        .combined_dependencies(Some(platform))
        .into_specs()
        .map(|(package_name, spec)| Dependency::Input(package_name, spec, "<environment>".into()))
        .collect_vec();

    if environment_dependencies.is_empty() && !locked_pixi_records.is_empty() {
        return Err(Box::new(PlatformUnsat::TooManyCondaPackages(Vec::new())));
    }

    // retrieve dependency-overrides
    // map it to (name => requirement) for later matching
    let dependency_overrides = environment
        .pypi_options()
        .dependency_overrides
        .unwrap_or_default()
        .into_iter()
        .map(|(name, req)| -> Result<_, Box<PlatformUnsat>> {
            let uv_req = as_uv_req(&req, name.as_source(), project_root).map_err(|e| {
                Box::new(PlatformUnsat::AsPep508Error(
                    name.as_normalized().clone(),
                    e,
                ))
            })?;
            Ok((uv_req.name.clone(), uv_req))
        })
        .collect::<Result<indexmap::IndexMap<_, _>, _>>()?;

    // Transform from PyPiPackage name into UV Requirement type
    let pypi_requirements = environment
        .pypi_dependencies(Some(platform))
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
    let virtual_packages = environment
        .virtual_packages(platform)
        .into_iter()
        .map(GenericVirtualPackage::from)
        .map(|vpkg| (vpkg.name.clone(), vpkg))
        .collect::<HashMap<_, _>>();

    // Find the python interpreter from the list of conda packages. Note that this
    // refers to the locked python interpreter, it might not match the specs
    // from the environment. That is ok because we will find that out when we
    // check all the records.
    let python_interpreter_record = locked_pixi_records.python_interpreter_record();

    // Determine the marker environment from the python interpreter package.
    let marker_environment = python_interpreter_record
        .map(|interpreter| determine_marker_environment(platform, &interpreter.package_record))
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
    let mut conda_queue = environment_dependencies;
    let mut pypi_queue = pypi_requirements;
    let mut expected_editable_pypi_packages = HashSet::new();
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
                            MatchSpec::from_nameless(spec, Some(name)),
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

                    if !identifier.satisfies(&requirement)? {
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
                                if let Err(err) =
                                    pypi_satifisfies_editable(&requirement, &record.0, project_root)
                                {
                                    delayed_pypi_error.get_or_insert(err);
                                }

                                // Record that we want this package to be editable. This is used to
                                // check at the end if packages that should be editable are actually
                                // editable and vice versa.
                                expected_editable_pypi_packages.insert(requirement.name.clone());

                                FoundPackage::PyPi(PypiPackageIdx(idx), requirement.extras.to_vec())
                            } else {
                                if let Err(err) = pypi_satifisfies_requirement(
                                    &requirement,
                                    &record.0,
                                    project_root,
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
                                &record.source
                            )),
                            SourceSpec::from(record.source.clone()).into(),
                        ),
                    };

                    if let Some((source, package_name)) = record
                        .as_source()
                        .and_then(|record| Some((record, spec.name.as_ref()?)))
                        .and_then(|(record, package_name)| {
                            Some((
                                record.sources.get(package_name.as_normalized())?,
                                package_name,
                            ))
                        })
                    {
                        let anchored_source = anchor.resolve(source.clone());
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
                            Cow::Owned(project_root.join(Path::new(path.as_str())))
                        };

                        if absolute_path.is_dir() {
                            match PypiSourceTreeHashable::from_directory(&absolute_path)
                                .map(|hashable| hashable.hash())
                            {
                                Ok(hashable) if Some(&hashable) != record.0.hash.as_ref() => {
                                    delayed_pypi_error.get_or_insert_with(|| {
                                        Box::new(PlatformUnsat::SourceTreeHashMismatch(
                                            record.0.name.clone(),
                                            SourceTreeHashMismatch {
                                                computed: hashable,
                                                locked: record.0.hash.clone(),
                                            },
                                        ))
                                    });
                                }
                                Ok(_) => {}
                                Err(err) => {
                                    delayed_pypi_error.get_or_insert_with(|| {
                                        Box::new(PlatformUnsat::FailedToDetermineSourceTreeHash(
                                            record.0.name.clone(),
                                            err,
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

    // Check if all source packages are still up-to-date.
    for source_record in locked_pixi_records
        .records
        .iter()
        .filter_map(PixiRecord::as_source)
    {
        let Some(path_record) = source_record.source.as_path() else {
            continue;
        };

        let Some(locked_input_hash) = &source_record.input_hash else {
            continue;
        };

        let source_dir = path_record.resolve(project_root);
        let source_dir = source_dir.canonicalize().map_err(|e| {
            Box::new(PlatformUnsat::FailedToCanonicalizePath(
                path_record.path.as_str().into(),
                e,
            ))
        })?;

        let discovered_backend = DiscoveredBackend::discover(
            &source_dir,
            &environment.channel_config(),
            &EnabledProtocols::default(),
        )
        .map_err(PlatformUnsat::BackendDiscovery)
        .map_err(Box::new)?;

        let project_model_hash = discovered_backend
            .init_params
            .project_model
            .as_ref()
            .map(compute_project_model_hash);

        let input_hash = input_hash_cache
            .compute_hash(GlobHashKey::new(
                source_dir,
                locked_input_hash.globs.clone(),
                project_model_hash,
            ))
            .await
            .map_err(PlatformUnsat::FailedToComputeInputHash)
            .map_err(Box::new)?;

        if input_hash.hash != locked_input_hash.hash {
            return Err(Box::new(PlatformUnsat::InputHashMismatch(
                path_record.path.to_string(),
                format!("{:x}", input_hash.hash),
                format!("{:x}", locked_input_hash.hash),
            )));
        }
    }

    // Now that we checked all conda requirements, check if there were any pypi issues.
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

    // Check if all packages that should be editable are actually editable and vice
    // versa.
    let locked_editable_packages = locked_pypi_environment
        .records
        .iter()
        .filter(|record| record.0.editable)
        .map(|record| {
            uv_normalize::PackageName::from_str(record.0.name.as_ref())
                .expect("cannot convert name")
        })
        .collect::<HashSet<_>>();
    let expected_editable = expected_editable_pypi_packages.sub(&locked_editable_packages);
    let unexpected_editable = locked_editable_packages.sub(&expected_editable_pypi_packages);
    if !expected_editable.is_empty() || !unexpected_editable.is_empty() {
        return Err(Box::new(PlatformUnsat::EditablePackageMismatch(
            EditablePackagesMismatch {
                expected_editable: expected_editable.into_iter().sorted().collect(),
                unexpected_editable: unexpected_editable.into_iter().sorted().collect(),
            },
        )));
    }

    Ok(VerifiedIndividualEnvironment {
        expected_conda_packages,
        conda_packages_used_by_pypi,
    })
}

enum FoundPackage {
    Conda(CondaPackageIdx),
    PyPi(PypiPackageIdx, Vec<uv_pep508::ExtraName>),
}

/// An index into the list of conda packages.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
#[repr(transparent)]
pub struct CondaPackageIdx(usize);

/// An index into the list of pypi packages.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
#[repr(transparent)]
pub struct PypiPackageIdx(usize);

fn find_matching_package(
    locked_pixi_records: &PixiRecordsByName,
    virtual_packages: &HashMap<rattler_conda_types::PackageName, GenericVirtualPackage>,
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
        Some(name) => {
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
    name: rattler_conda_types::PackageName,
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
        .source
        .satisfies(&source_spec)
        .map_err(|e| PlatformUnsat::SourcePackageMismatch(name.as_source().to_string(), e))?;

    if let Some(match_spec) = match_spec {
        if !match_spec.matches(package) {
            return Err(Box::new(PlatformUnsat::UnsatisfiableMatchSpec(
                Box::new(match_spec),
                source.into_owned(),
            )));
        }
    }

    Ok(CondaPackageIdx(idx))
}

trait MatchesMatchspec {
    fn matches(&self, spec: &MatchSpec) -> bool;
}

impl MatchesMatchspec for GenericVirtualPackage {
    fn matches(&self, spec: &MatchSpec) -> bool {
        if let Some(name) = &spec.name {
            if name != &self.name {
                return false;
            }
        }

        if let Some(version) = &spec.version {
            if !version.matches(&self.version) {
                return false;
            }
        }

        if let Some(build) = &spec.build {
            if !build.matches(&self.build_string) {
                return false;
            }
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

impl Display for EditablePackagesMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.expected_editable.is_empty() && self.unexpected_editable.is_empty() {
            write!(f, "expected ")?;
            format_package_list(f, &self.expected_editable)?;
            write!(
                f,
                " to be editable but in the lock-file {they} {are} not",
                they = it_they(self.expected_editable.len()),
                are = is_are(self.expected_editable.len())
            )?
        } else if self.expected_editable.is_empty() && !self.unexpected_editable.is_empty() {
            write!(f, "expected ")?;
            format_package_list(f, &self.unexpected_editable)?;
            write!(
                f,
                "NOT to be editable but in the lock-file {they} {are}",
                they = it_they(self.unexpected_editable.len()),
                are = is_are(self.unexpected_editable.len())
            )?
        } else {
            write!(f, "expected ")?;
            format_package_list(f, &self.expected_editable)?;
            write!(
                f,
                " to be editable but in the lock-file but {they} {are} not, whereas ",
                they = it_they(self.expected_editable.len()),
                are = is_are(self.expected_editable.len())
            )?;
            format_package_list(f, &self.unexpected_editable)?;
            write!(
                f,
                " {are} NOT expected to be editable which in the lock-file {they} {are}",
                they = it_they(self.unexpected_editable.len()),
                are = is_are(self.unexpected_editable.len())
            )?
        }

        return Ok(());

        fn format_package_list(
            f: &mut std::fmt::Formatter<'_>,
            packages: &[uv_normalize::PackageName],
        ) -> std::fmt::Result {
            for (idx, package) in packages.iter().enumerate() {
                if idx == packages.len() - 1 && idx > 0 {
                    write!(f, " and ")?;
                } else if idx > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{}", package)?;
            }

            Ok(())
        }

        fn is_are(count: usize) -> &'static str {
            if count == 1 { "is" } else { "are" }
        }

        fn it_they(count: usize) -> &'static str {
            if count == 1 { "it" } else { "they" }
        }
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
    use rattler_lock::LockFile;
    use rstest::rstest;

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
    ) -> Result<(), LockfileUnsat> {
        let mut individual_verified_envs = HashMap::new();

        // Verify individual environment satisfiability
        for env in project.environments() {
            let locked_env = lock_file
                .environment(env.name().as_str())
                .ok_or_else(|| LockfileUnsat::EnvironmentMissing(env.name().to_string()))?;
            verify_environment_satisfiability(&env, locked_env)
                .map_err(|e| LockfileUnsat::Environment(env.name().to_string(), e))?;

            for platform in env.platforms() {
                let verified_env = verify_platform_satisfiability(
                    &env,
                    locked_env,
                    platform,
                    project.root(),
                    Default::default(),
                )
                .await
                .map_err(|e| LockfileUnsat::PlatformUnsat(env.name().to_string(), platform, *e))?;

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
        match verify_lockfile_satisfiability(&project, &lock_file)
            .await
            .into_diagnostic()
        {
            Ok(()) => {}
            Err(e) => panic!("{e:?}"),
        }
    }

    #[rstest]
    #[tokio::test]
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
        match verify_lockfile_satisfiability(&project, &lock_file)
            .await
            .into_diagnostic()
        {
            Ok(()) => {}
            Err(e) => panic!("{e:?}"),
        }
    }

    #[rstest]
    #[tokio::test]
    async fn test_failing_satisiability(
        #[files("../../tests/data/non-satisfiability/*/pixi.toml")] manifest_path: PathBuf,
    ) {
        let report_handler = NarratableReportHandler::new().with_cause_chain();

        let project = Workspace::from_path(&manifest_path).unwrap();
        let lock_file = LockFile::from_path(&project.lock_file_path()).unwrap();
        let err = verify_lockfile_satisfiability(&project, &lock_file)
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
