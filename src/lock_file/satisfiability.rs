use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fmt::{Display, Formatter},
    ops::Sub,
    path::{Path, PathBuf},
    str::FromStr,
};

use itertools::{Either, Itertools};
use miette::Diagnostic;
use pep440_rs::VersionSpecifiers;
use pixi_glob::{GlobHashCache, GlobHashError, GlobHashKey};
use pixi_manifest::FeaturesExt;
use pixi_record::{ParseLockFileError, PixiRecord, SourceMismatchError};
use pixi_spec::{PixiSpec, SourceSpec, SpecConversionError};
use pixi_uv_conversions::{
    as_uv_req, to_normalize, to_uv_marker_tree, to_uv_version_specifiers, AsPep508Error,
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
use url::Url;
use uv_distribution_filename::DistExtension;
use uv_git::GitReference;
use uv_pypi_types::{
    ParsedGitUrl, ParsedPathUrl, ParsedUrl, ParsedUrlError, RequirementSource, VerbatimParsedUrl,
};

use super::{
    package_identifier::ConversionError, PixiRecordsByName, PypiRecord, PypiRecordsByName,
};
use crate::project::{grouped_environment::GroupedEnvironment, Environment, HasProjectRef};

#[derive(Debug, Error, Diagnostic)]
pub enum EnvironmentUnsat {
    #[error("the channels in the lock-file do not match the environments channels")]
    ChannelsMismatch,

    #[error(transparent)]
    IndexesMismatch(#[from] IndexesMismatch),

    #[error(transparent)]
    InvalidChannel(#[from] ParseChannelError),
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
                write!(f,
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
    UnsatisfiableMatchSpec(MatchSpec, String),

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
    UnsatisfiableRequirement(Box<uv_pypi_types::Requirement>, String),

    #[error("the conda package does not satisfy the pypi requirement '{0}' (required by '{1}')")]
    CondaUnsatisfiableRequirement(Box<uv_pypi_types::Requirement>, String),

    #[error("there was a duplicate entry for '{0}'")]
    DuplicateEntry(String),

    #[error("the requirement '{0}' failed to parse")]
    FailedToParseMatchSpec(String, #[source] ParseMatchSpecError),

    #[error("there are more conda packages in the lock-file than are used by the environment")]
    TooManyCondaPackages,

    #[error("missing purls")]
    MissingPurls,

    #[error("corrupted lock-file entry for '{0}'")]
    CorruptedEntry(String, ParseLockFileError),

    #[error("there are more pypi packages in the lock-file than are used by the environment: {}", .0.iter().format(", "))]
    TooManyPypiPackages(Vec<pep508_rs::PackageName>),

    #[error("there are PyPi dependencies but a python interpreter is missing from the lock-file")]
    MissingPythonInterpreter,

    #[error(
        "a marker environment could not be derived from the python interpreter in the lock-file"
    )]
    FailedToDetermineMarkerEnvironment(#[source] Box<dyn Diagnostic + Send + Sync>),

    #[error("{0} requires python version {1} but the python interpreter in the lock-file has version {2}")]
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
        Box<uv_pypi_types::RequirementSource>,
    ),

    #[error("direct pypi url dependency to a conda installed package '{0}' is not supported")]
    DirectUrlDependencyOnCondaInstalledPackage(uv_normalize::PackageName),

    #[error("git dependency on a conda installed package '{0}' is not supported")]
    GitDependencyOnCondaInstalledPackage(uv_normalize::PackageName),

    #[error(transparent)]
    EditablePackageMismatch(EditablePackagesMismatch),

    #[error("the editable package '{0}' was expected to be a directory but is a url, which cannot be editable: '{1}'")]
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

    #[error("'{name}' expected this path {expected_path} but found {found_path}")]
    LockedPyPIPathMismatch {
        name: String,
        expected_path: PathBuf,
        found_path: PathBuf,
    },

    #[error("failed to convert between pep508 and uv types {0}")]
    UvTypesConversionError(#[from] ConversionError),
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

/// Convert something into a uv requirement.
trait IntoUvRequirement {
    type E;
    fn into_uv_requirement(self) -> Result<uv_pypi_types::Requirement, Self::E>;
}

impl IntoUvRequirement for pep508_rs::Requirement {
    type E = ConversionError;

    fn into_uv_requirement(self) -> Result<uv_pypi_types::Requirement, Self::E> {
        let parsed_url = if let Some(version_or_url) = self.version_or_url {
            match version_or_url {
                pep508_rs::VersionOrUrl::VersionSpecifier(version) => Some(
                    uv_pep508::VersionOrUrl::VersionSpecifier(to_uv_version_specifiers(&version)?),
                ),
                pep508_rs::VersionOrUrl::Url(verbatim_url) => {
                    let url_or_path =
                        UrlOrPath::from_str(verbatim_url.as_str()).expect("should be convertible");

                    // it is actually a path
                    let url = match url_or_path {
                        UrlOrPath::Path(path) => {
                            let ext = DistExtension::from_path(Path::new(path.as_str()))
                                .map_err(|e| {
                                    ParsedUrlError::MissingExtensionPath(
                                        PathBuf::from_str(path.as_str()).expect("not a path"),
                                        e,
                                    )
                                })
                                .expect("cannot get extension");
                            let parsed_url = ParsedUrl::Path(ParsedPathUrl::from_source(
                                path.as_str().into(),
                                ext,
                                verbatim_url.to_url(),
                            ));

                            VerbatimParsedUrl {
                                parsed_url,
                                verbatim: uv_pep508::VerbatimUrl::from_url(
                                    verbatim_url.raw().clone(),
                                )
                                .with_given(verbatim_url.given().unwrap()),
                            }
                            // Can only be an archive
                        }
                        UrlOrPath::Url(u) => VerbatimParsedUrl {
                            parsed_url: ParsedUrl::try_from(u.clone())
                                .expect("cannot convert to url"),
                            verbatim: uv_pep508::VerbatimUrl::from_url(u),
                        },
                    };

                    Some(uv_pep508::VersionOrUrl::Url(url))
                }
            }
        } else {
            None
        };

        let marker = to_uv_marker_tree(&self.marker)?;
        let converted = uv_pep508::Requirement {
            name: uv_pep508::PackageName::new(self.name.to_string())
                .expect("cannot normalize name"),
            extras: self
                .extras
                .iter()
                .map(|e| {
                    uv_pep508::ExtraName::new(e.to_string()).expect("cannot convert extra name")
                })
                .collect(),
            marker,
            version_or_url: parsed_url,
            // Don't think this needs to be set
            origin: None,
        };

        Ok(converted.into())
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
    let config = environment.project().channel_config();
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

    // Check if the indexes in the lock file match our current configuration.
    if !environment.pypi_dependencies(None).is_empty() {
        let indexes = rattler_lock::PypiIndexes::from(grouped_env.pypi_options());
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
) -> Result<(), Box<PlatformUnsat>> {
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
            )))
        }
    };

    // Create a lookup table from package name to package record. Returns an error
    // if we find a duplicate entry for a record
    let pypi_records_by_name = match PypiRecordsByName::from_unique_iter(pypi_packages) {
        Ok(pypi_packages) => pypi_packages,
        Err(duplicate) => {
            return Err(Box::new(PlatformUnsat::DuplicateEntry(
                duplicate.0.name.to_string(),
            )))
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
    PyPi(uv_pypi_types::Requirement, Cow<'static, str>),
}

/// Check satatisfiability of a pypi requirement against a locked pypi package
/// This also does an additional check for git urls when using direct url
/// references
pub(crate) fn pypi_satifisfies_editable(
    spec: &uv_pypi_types::Requirement,
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

                if &canocalized_path != install_path {
                    return Err(Box::new(PlatformUnsat::EditablePackagePathMismatch(
                        spec.name.clone(),
                        absolute_path.into_owned(),
                        install_path.clone(),
                    )));
                }
                Ok(())
            }
        },
    }
}

/// Checks if the string seems like a git commit sha
fn seems_like_commit_sha(s: &str) -> bool {
    s.len() >= 4 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Check satatisfiability of a pypi requirement against a locked pypi package
/// This also does an additional check for git urls when using direct url
/// references
pub(crate) fn pypi_satifisfies_requirement(
    spec: &uv_pypi_types::Requirement,
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

                if *spec_url.raw() == locked_url {
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
            repository,
            reference,
            precise: _precise,
            ..
        } => {
            match &locked_data.location {
                UrlOrPath::Url(url) => {
                    if let Ok(locked_git_url) = ParsedGitUrl::try_from(url.clone()) {
                        let repo_is_same = locked_git_url.url.repository() == repository;
                        if !repo_is_same {
                            return Err(PlatformUnsat::LockedPyPIGitUrlMismatch {
                                name: spec.name.clone().to_string(),
                                spec_url: repository.to_string(),
                                lock_url: locked_git_url.url.repository().to_string(),
                            }
                            .into());
                        }
                        // If the spec does not specify a revision than any will do
                        // E.g `git.com/user/repo` is the same as `git.com/user/repo@adbdd`
                        if *reference == GitReference::DefaultBranch {
                            return Ok(());
                        }
                        // If the spec has a short commit than we can do a partial match
                        // E.g `git.com/user/repo@adbdd` is the same as `git.com/user/repo@adbdd123`
                        // Currently this resolves to BranchOrTag
                        if let GitReference::BranchOrTag(ref branch_or_tag) = reference {
                            if seems_like_commit_sha(branch_or_tag) {
                                // We expect the lock file to have a long commit hash
                                // in this case
                                if let GitReference::FullCommit(sha) =
                                    locked_git_url.url.reference()
                                {
                                    if sha.starts_with(branch_or_tag) {
                                        return Ok(());
                                    } else {
                                        return Err(PlatformUnsat::LockedPyPIGitRefMismatch {
                                            name: spec.name.clone().to_string(),
                                            expected_ref: branch_or_tag.to_string(),
                                            found_ref: sha.to_string(),
                                        }
                                        .into());
                                    }
                                }
                            }
                        }

                        // If the spec does specify a revision than the revision must match
                        if locked_git_url.url.reference() == reference {
                            return Ok(());
                        } else {
                            return Err(PlatformUnsat::LockedPyPIGitRefMismatch {
                                name: spec.name.clone().to_string(),
                                expected_ref: reference.to_string(),
                                found_ref: locked_git_url.url.reference().to_string(),
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
                let locked_path = Path::new(locked_path.as_str());
                // sometimes the path is relative, so we need to join it with the project root
                if &project_root.join(locked_path) != install_path {
                    return Err(PlatformUnsat::LockedPyPIPathMismatch {
                        name: spec.name.clone().to_string(),
                        expected_path: install_path.clone(),
                        found_path: project_root.join(locked_path),
                    }
                    .into());
                }
                return Ok(());
            }
            Err(PlatformUnsat::LockedPyPIRequiresPath(spec.name.to_string()).into())
        }
    }
}

pub(crate) async fn verify_package_platform_satisfiability(
    environment: &Environment<'_>,
    locked_pixi_records: &PixiRecordsByName,
    locked_pypi_environment: &PypiRecordsByName,
    platform: Platform,
    project_root: &Path,
    input_hash_cache: GlobHashCache,
) -> Result<(), Box<PlatformUnsat>> {
    let channel_config = environment.project().channel_config();

    // Determine the dependencies requested by the environment
    let environment_dependencies = environment
        .combined_dependencies(Some(platform))
        .into_specs()
        .map(|(package_name, spec)| Dependency::Input(package_name, spec, "<environment>".into()))
        .collect_vec();

    if environment_dependencies.is_empty() && !locked_pixi_records.is_empty() {
        return Err(Box::new(PlatformUnsat::TooManyCondaPackages));
    }

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
    while let Some(package) = conda_queue.pop().or_else(|| pypi_queue.pop()) {
        // Determine the package that matches the requirement of matchspec.
        let found_package = match package {
            Dependency::Input(name, spec, source) => {
                match spec.into_source_or_binary(&channel_config) {
                    Ok(Either::Left(source_spec)) => {
                        expected_conda_source_dependencies.insert(name.clone());
                        find_matching_source_package(
                            locked_pixi_records,
                            name,
                            source_spec,
                            source,
                        )?
                    }
                    Ok(Either::Right(spec)) => {
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
                            SpecConversionError::InvalidChannel(p) => p.into(),
                        };
                        return Err(Box::new(PlatformUnsat::FailedToParseMatchSpec(
                            name.as_source().to_string(),
                            parse_channel_err,
                        )));
                    }
                }
            }
            Dependency::Conda(spec, source) => {
                match find_matching_package(locked_pixi_records, &virtual_packages, spec, source)? {
                    Some(pkg) => pkg,
                    None => continue,
                }
            }
            Dependency::PyPi(requirement, source) => {
                // Check if there is a pypi identifier that matches our requirement.
                if let Some((identifier, repodata_idx, _)) =
                    locked_conda_pypi_packages.get(&requirement.name)
                {
                    if requirement.is_editable() {
                        return Err(Box::new(
                            PlatformUnsat::EditableDependencyOnCondaInstalledPackage(
                                requirement.name.clone(),
                                Box::new(requirement.source),
                            ),
                        ));
                    }

                    if matches!(requirement.source, RequirementSource::Url { .. }) {
                        return Err(Box::new(
                            PlatformUnsat::DirectUrlDependencyOnCondaInstalledPackage(
                                requirement.name.clone(),
                            ),
                        ));
                    }

                    if matches!(requirement.source, RequirementSource::Git { .. }) {
                        return Err(Box::new(
                            PlatformUnsat::GitDependencyOnCondaInstalledPackage(
                                requirement.name.clone(),
                            ),
                        ));
                    }

                    if !identifier
                        .satisfies(&requirement)
                        .map_err(From::from)
                        .map_err(Box::new)?
                    {
                        // The record does not match the spec, the lock-file is inconsistent.
                        return Err(Box::new(PlatformUnsat::CondaUnsatisfiableRequirement(
                            Box::new(requirement.clone()),
                            source.into_owned(),
                        )));
                    }
                    FoundPackage::Conda(*repodata_idx)
                } else if let Some(idx) = locked_pypi_environment.index_by_name(
                    &to_normalize(&requirement.name)
                        .map_err(ConversionError::NameConversion)
                        .map_err(From::from)
                        .map_err(Box::new)?,
                ) {
                    let record = &locked_pypi_environment.records[idx];
                    if requirement.is_editable() {
                        pypi_satifisfies_editable(&requirement, &record.0, project_root)?;

                        // Record that we want this package to be editable. This is used to
                        // check at the end if packages that should be editable are actually
                        // editable and vice versa.
                        expected_editable_pypi_packages.insert(requirement.name.clone());

                        FoundPackage::PyPi(idx, requirement.extras)
                    } else {
                        pypi_satifisfies_requirement(&requirement, &record.0, project_root)?;
                        FoundPackage::PyPi(idx, requirement.extras)
                    }
                } else {
                    // The record does not match the spec, the lock-file is inconsistent.
                    return Err(Box::new(PlatformUnsat::UnsatisfiableRequirement(
                        Box::new(requirement),
                        source.into_owned(),
                    )));
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

                let record = &locked_pixi_records.records[idx];
                for depends in &record.package_record().depends {
                    let spec = MatchSpec::from_str(depends.as_str(), Lenient)
                        .map_err(|e| PlatformUnsat::FailedToParseMatchSpec(depends.clone(), e))?;
                    conda_queue.push(Dependency::Conda(
                        spec,
                        match record {
                            PixiRecord::Binary(record) => Cow::Owned(record.file_name.to_string()),
                            PixiRecord::Source(record) => Cow::Owned(format!(
                                "{} @ {}",
                                record.package_record.name.as_source(),
                                &record.source
                            )),
                        },
                    ));
                }
            }
            FoundPackage::PyPi(idx, extras) => {
                let record = &locked_pypi_environment.records[idx];

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
                            let hashable = PypiSourceTreeHashable::from_directory(&absolute_path)
                                .map_err(|e| {
                                    PlatformUnsat::FailedToDetermineSourceTreeHash(
                                        record.0.name.clone(),
                                        e,
                                    )
                                })?
                                .hash();
                            if Some(&hashable) != record.0.hash.as_ref() {
                                return Err(Box::new(PlatformUnsat::SourceTreeHashMismatch(
                                    record.0.name.clone(),
                                    SourceTreeHashMismatch {
                                        computed: hashable,
                                        locked: record.0.hash.clone(),
                                    },
                                )));
                            }
                        }
                    }

                    // Ensure that the record matches the currently selected interpreter.
                    if let Some(python_version) = &record.0.requires_python {
                        let marker_version = pep440_rs::Version::from_str(
                            &marker_environment.python_full_version().version.to_string(),
                        )
                        .expect("cannot parse version");
                        if !python_version.contains(&marker_version) {
                            return Err(Box::new(PlatformUnsat::PythonVersionMismatch(
                                record.0.name.clone(),
                                python_version.clone(),
                                marker_version.into(),
                            )));
                        }
                    }
                }

                // Add all the requirements of the package to the queue.
                for requirement in &record.0.requires_dist {
                    let requirement = requirement
                        .clone()
                        .into_uv_requirement()
                        .map_err(From::from)
                        .map_err(Box::new)?;
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

    // Check if all locked packages have also been visisted
    if conda_packages_visited.len() != locked_pixi_records.len() {
        return Err(Box::new(PlatformUnsat::TooManyCondaPackages));
    }

    if pypi_packages_visited.len() != locked_pypi_environment.len() {
        return Err(Box::new(PlatformUnsat::TooManyPypiPackages(
            locked_pypi_environment
                .names()
                .enumerate()
                .filter_map(|(idx, name)| {
                    if pypi_packages_visited.contains(&idx) {
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
            uv_normalize::PackageName::new(record.0.name.to_string()).expect("cannot convert name")
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

        let input_hash = input_hash_cache
            .compute_hash(GlobHashKey {
                root: source_dir,
                globs: locked_input_hash.globs.clone(),
            })
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

    Ok(())
}

enum FoundPackage {
    Conda(usize),
    PyPi(usize, Vec<uv_pep508::ExtraName>),
}

fn find_matching_package(
    locked_pixi_records: &PixiRecordsByName,
    virtual_packages: &HashMap<rattler_conda_types::PackageName, GenericVirtualPackage>,
    spec: MatchSpec,
    source: Cow<str>,
) -> Result<Option<FoundPackage>, Box<PlatformUnsat>> {
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
                        spec,
                        source.into_owned(),
                    )));
                }
                Some(idx) => FoundPackage::Conda(idx),
            }
        }
        Some(name) => {
            match locked_pixi_records
                .index_by_name(name)
                .map(|idx| (idx, &locked_pixi_records.records[idx]))
            {
                Some((idx, record)) if spec.matches(record) => FoundPackage::Conda(idx),
                Some(_) => {
                    // The record does not match the spec, the lock-file is
                    // inconsistent.
                    return Err(Box::new(PlatformUnsat::UnsatisfiableMatchSpec(
                        spec,
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
                                spec,
                                source.into_owned(),
                            )));
                        }
                    } else {
                        // The record does not match the spec, the lock-file is
                        // inconsistent.
                        return Err(Box::new(PlatformUnsat::UnsatisfiableMatchSpec(
                            spec,
                            source.into_owned(),
                        )));
                    }
                }
            }
        }
    };

    Ok(Some(found_package))
}

fn find_matching_source_package(
    locked_pixi_records: &PixiRecordsByName,
    name: rattler_conda_types::PackageName,
    source_spec: SourceSpec,
    source: Cow<str>,
) -> Result<FoundPackage, Box<PlatformUnsat>> {
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

    Ok(FoundPackage::Conda(idx))
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
            if count == 1 {
                "is"
            } else {
                "are"
            }
        }

        fn it_they(count: usize) -> &'static str {
            if count == 1 {
                "it"
            } else {
                "they"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use insta::Settings;
    use miette::{IntoDiagnostic, NarratableReportHandler};
    use pep440_rs::Version;
    use rattler_lock::LockFile;
    use rstest::rstest;

    use std::{
        ffi::OsStr,
        path::{Component, PathBuf},
        str::FromStr,
    };

    use super::*;
    use crate::Project;

    #[derive(Error, Debug, Diagnostic)]
    enum LockfileUnsat {
        #[error("environment '{0}' is missing")]
        EnvironmentMissing(String),

        #[error("environment '{0}' does not satisfy the requirements of the project")]
        Environment(String, #[source] EnvironmentUnsat),

        #[error(
            "environment '{0}' does not satisfy the requirements of the project for platform '{1}"
        )]
        PlatformUnsat(String, Platform, #[source] PlatformUnsat),
    }

    async fn verify_lockfile_satisfiability(
        project: &Project,
        lock_file: &LockFile,
    ) -> Result<(), LockfileUnsat> {
        for env in project.environments() {
            let locked_env = lock_file
                .environment(env.name().as_str())
                .ok_or_else(|| LockfileUnsat::EnvironmentMissing(env.name().to_string()))?;
            verify_environment_satisfiability(&env, locked_env)
                .map_err(|e| LockfileUnsat::Environment(env.name().to_string(), e))?;

            for platform in env.platforms() {
                verify_platform_satisfiability(
                    &env,
                    locked_env,
                    platform,
                    project.root(),
                    Default::default(),
                )
                .await
                .map_err(|e| LockfileUnsat::PlatformUnsat(env.name().to_string(), platform, *e))?;
            }
        }
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn test_good_satisfiability(
        #[files("tests/data/satisfiability/*/pixi.toml")] manifest_path: PathBuf,
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

        let project = Project::from_path(&manifest_path).unwrap();
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
    #[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
    async fn test_example_satisfiability(#[files("examples/*/pixi.toml")] manifest_path: PathBuf) {
        let project = Project::from_path(&manifest_path).unwrap();
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
        #[files("tests/data/non-satisfiability/*/pixi.toml")] manifest_path: PathBuf,
    ) {
        let report_handler = NarratableReportHandler::new().with_cause_chain();

        let project = Project::from_path(&manifest_path).unwrap();
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
        // Mock locked datga
        let locked_data = PypiPackageData {
            name: "mypkg".parse().unwrap(),
            version: Version::from_str("0.1.0").unwrap(),
            location: "git+https://github.com/mypkg@29932f3915935d773dc8d52c292cadd81c81071d"
                .parse()
                .expect("failed to parse url"),
            hash: None,
            requires_dist: vec![],
            requires_python: None,
            editable: false,
        };
        let spec = pep508_rs::Requirement::from_str("mypkg @ git+https://github.com/mypkg@2993")
            .unwrap()
            .into_uv_requirement()
            .unwrap();
        let project_root = PathBuf::from_str("/").unwrap();
        // This should satisfy:
        pypi_satifisfies_requirement(&spec, &locked_data, &project_root).unwrap();
        let non_matching_spec =
            pep508_rs::Requirement::from_str("mypkg @ git+https://github.com/mypkg@defgd")
                .unwrap()
                .into_uv_requirement()
                .unwrap();
        // This should not
        pypi_satifisfies_requirement(&non_matching_spec, &locked_data, &project_root).unwrap_err();
        // Removing the rev from the Requirement should satisfy any revision
        let spec = pep508_rs::Requirement::from_str("mypkg @ git+https://github.com/mypkg")
            .unwrap()
            .into_uv_requirement()
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

        let spec = spec.into_uv_requirement().unwrap();

        // This should satisfy:
        pypi_satifisfies_requirement(&spec, &locked_data, Path::new("")).unwrap();
    }
}
