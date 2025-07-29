use uv_distribution_types::{CachedDist, Dist, InstalledDist};

use super::InstallReason;

#[derive(Debug)]
pub struct PyPIInstallationPlan {
    /// The distributions that are not already installed in the current
    /// environment, but are available in the local cache.
    pub cached: Vec<(CachedDist, InstallReason)>,

    /// The distributions that are not already installed in the current
    /// environment, and are not available in the local cache.
    /// this is where we differ from UV because we want already have the URL we
    /// want to download
    pub remote: Vec<(Dist, InstallReason)>,

    /// Any distributions that are already installed in the current environment,
    /// but will be re-installed (including upgraded) to satisfy the
    /// requirements.
    pub reinstalls: Vec<(InstalledDist, NeedReinstall)>,

    /// Extraneous packages that need to be removed
    pub extraneous: Vec<InstalledDist>,

    /// Duplicates are packages that are extraneous but have different versions
    /// among PyPI and conda requirements, we only want to remove the metadata
    /// for these
    pub duplicates: Vec<InstalledDist>,
}

/// Represents the different reasons why a package needs to be reinstalled
#[derive(Debug)]
pub(crate) enum NeedReinstall {
    /// The package is not installed
    VersionMismatch {
        installed_version: uv_pep440::Version,
        locked_version: pep440_rs::Version,
    },
    /// The `direct_url.json` file is missing
    MissingDirectUrl,
    /// The source directory is newer than the cache, requires a rebuild
    SourceDirectoryNewerThanCache,
    /// Url file parse error
    UnableToParseFileUrl { url: String },
    /// Unable to convert locked directory to a url
    UnableToConvertLockedPath { path: String },
    /// The editable status of the installed wheel changed with regards to the locked version
    EditableStatusChanged {
        locked_editable: bool,
        installed_editable: bool,
    },
    /// Somehow unable to parse the installed dist url
    UnableToParseInstalledDistUrl { url: String },
    /// Archive is newer than the cache
    ArchiveDistNewerThanCache,
    /// The git archive is still path, could be caused by an old source install
    GitArchiveIsPath,
    /// The git revision is different from the locked version
    GitRevMismatch {
        installed_rev: String,
        locked_rev: String,
    },
    /// Unable to parse the installed git url
    UnableToParseGitUrl { url: String },
    /// Unable to get the installed dist metadata, something is definitely broken
    UnableToGetInstalledDistMetadata { cause: String },
    /// The to install requires-python is different from the installed version
    RequiredPythonChanged {
        installed_python_require: String,
        locked_python_version: String,
    },
    /// Re-installing because of an installer mismatch, but we are managing the package
    InstallerMismatch { previous_installer: String },
    /// The installed url does not match the locked url
    UrlMismatch {
        installed_url: String,
        locked_url: Option<String>,
    },
    /// Package is installed by registry, but we want a non registry location.
    SourceMismatch {
        locked_location: String,
        installed_location: String,
    },
    /// Reinstallation was requested
    ReinstallationRequested,
}

impl std::fmt::Display for NeedReinstall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NeedReinstall::VersionMismatch {
                installed_version,
                locked_version,
            } => write!(
                f,
                "Installed version {} does not match locked version {}",
                installed_version, locked_version
            ),
            NeedReinstall::MissingDirectUrl => write!(f, "Missing direct_url.json"),
            NeedReinstall::SourceDirectoryNewerThanCache => {
                write!(f, "Source directory is newer than the cache")
            }
            NeedReinstall::UnableToParseFileUrl { url } => {
                write!(f, "Unable to parse file url: {}", url)
            }
            NeedReinstall::EditableStatusChanged {
                locked_editable,
                installed_editable,
            } => {
                write!(
                    f,
                    "Editable status changed, editable status is: {} installed editable is: {}",
                    locked_editable, installed_editable
                )
            }
            NeedReinstall::UnableToParseInstalledDistUrl { url } => {
                write!(f, "Unable to parse installed dist url: {}", url)
            }
            NeedReinstall::ArchiveDistNewerThanCache => {
                write!(f, "Archive dist is newer than the cache")
            }
            NeedReinstall::GitArchiveIsPath => write!(f, "Git archive is a path"),
            NeedReinstall::GitRevMismatch {
                installed_rev: installed_commit,
                locked_rev: locked_commit,
            } => write!(
                f,
                "Git commits mismatch, installed commit: {}, locked commit: {}",
                installed_commit, locked_commit
            ),
            NeedReinstall::UnableToParseGitUrl { url } => {
                write!(f, "Unable to parse git url: {}", url)
            }
            NeedReinstall::UnableToGetInstalledDistMetadata { cause } => {
                write!(f, "Unable to get installed dist metadata: {}", cause)
            }
            NeedReinstall::RequiredPythonChanged {
                installed_python_require: installed_python_version,
                locked_python_version,
            } => {
                write!(
                    f,
                    "Installed requires-python {} does not contain locked python version {}",
                    installed_python_version, locked_python_version
                )
            }
            NeedReinstall::InstallerMismatch { previous_installer } => {
                write!(
                    f,
                    "Installer mismatch, previous installer: {}",
                    previous_installer
                )
            }
            NeedReinstall::UrlMismatch {
                installed_url,
                locked_url,
            } => write!(
                f,
                "Installed url {} does not match locked url {}",
                installed_url,
                locked_url.clone().unwrap_or_else(|| "None".to_string())
            ),
            NeedReinstall::UnableToConvertLockedPath { path } => {
                write!(f, "Unable to convert locked path to url: {}", path)
            }
            NeedReinstall::SourceMismatch {
                locked_location,
                installed_location,
            } => write!(
                f,
                "Installed from registry from '{installed_location}' but locked to a non-registry location from '{locked_location}'",
            ),
            NeedReinstall::ReinstallationRequested => write!(f, "Reinstallation was requested",),
        }
    }
}

pub(crate) enum ValidateCurrentInstall {
    /// Keep this package
    Keep,
    /// Reinstall this package
    Reinstall(NeedReinstall),
}
