use std::path::PathBuf;

use itertools::Itertools;
use miette::Diagnostic;
use pep440_rs::VersionSpecifiers;
use pixi_glob::GlobHashError;
use pixi_record::{ParseLockFileError, SourceMismatchError};
use pixi_uv_conversions::AsPep508Error;
use rattler_conda_types::{MatchSpec, ParseMatchSpecError};
use thiserror::Error;
use url::Url;
use uv_pypi_types::ParsedUrlError;

use crate::{
    package_identifier::ConversionError,
    satisfiability::{EditablePackagesMismatch, SourceTreeHashMismatch},
};

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

impl PlatformUnsat {
    /// Returns true if this is a problem with pypi packages only. This means
    /// the conda packages are still considered valid.
    pub fn is_pypi_only(&self) -> bool {
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
