use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use miette::IntoDiagnostic;
use pixi_consts::consts;
use pixi_uv_conversions::to_uv_version;
use rattler_lock::{PypiPackageData, UrlOrPath};
use url::Url;
use uv_cache::Cache;
use uv_distribution::RegistryWheelIndex;
use uv_distribution_types::{CachedDist, CachedRegistryDist, Dist, InstalledDist, Name};
use uv_installer::SitePackages;
use uv_pypi_types::ParsedGitUrl;

use super::{
    conversions::convert_to_dist,
    utils::{check_url_freshness, strip_direct_scheme},
};

#[derive(Debug)]
pub enum InstallReason {
    /// Reinstall a package from the local cache, will link from the cache
    ReinstallCached,
    /// Reinstall a package that we have determined to be stale, will be taken from the registry
    ReinstallStaleLocal,
    /// Reinstall a package that is missing from the local cache, but is available in the registry
    ReinstallMissing,
    /// Install a package from the local cache, will link from the cache
    InstallCached,
    /// Install a package that we have determined to be stale, will be taken from the registry
    InstallStaleLocal,
    /// Install a package that is missing from the local cache, but is available in the registry
    InstallMissing,
}

/// This trait can be used to generalize over the different reason why a specific installation source was chosen
/// So we can differentiate between re-installing and installing a package, this is all a bit verbose
/// but can be quite useful for debugging and logging
trait OperationToReason {
    /// This package is available in the local cache
    fn cached(&self) -> InstallReason;
    /// This package is determined to be stale
    fn stale(&self) -> InstallReason;
    /// This package is missing from the local cache
    fn missing(&self) -> InstallReason;
}

/// Use this struct to get the correct install reason
struct Install;
impl OperationToReason for Install {
    fn cached(&self) -> InstallReason {
        InstallReason::InstallCached
    }

    fn stale(&self) -> InstallReason {
        InstallReason::InstallStaleLocal
    }

    fn missing(&self) -> InstallReason {
        InstallReason::InstallMissing
    }
}

/// Use this struct to get the correct reinstall reason
struct Reinstall;
impl OperationToReason for Reinstall {
    fn cached(&self) -> InstallReason {
        InstallReason::ReinstallCached
    }

    fn stale(&self) -> InstallReason {
        InstallReason::ReinstallStaleLocal
    }

    fn missing(&self) -> InstallReason {
        InstallReason::ReinstallMissing
    }
}

/// Derived from uv [`uv_installer::Plan`]
#[derive(Debug)]
pub struct PixiInstallPlan {
    /// The distributions that are not already installed in the current
    /// environment, but are available in the local cache.
    pub local: Vec<(CachedDist, InstallReason)>,

    /// The distributions that are not already installed in the current
    /// environment, and are not available in the local cache.
    /// this is where we differ from UV because we want already have the URL we
    /// want to download
    pub remote: Vec<(Dist, InstallReason)>,

    /// Any distributions that are already installed in the current environment,
    /// but will be re-installed (including upgraded) to satisfy the
    /// requirements.
    pub reinstalls: Vec<(InstalledDist, NeedReinstall)>,

    /// Any distributions that are already installed in the current environment,
    /// and are _not_ necessary to satisfy the requirements.
    pub extraneous: Vec<InstalledDist>,
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
    /// The editable status of the installed wheel changed with regards to the locked version
    EditableStatusChanged { is_now_editable: bool },
    /// Somehow unable to parse the installed dist url
    UnableToParseInstalledDistUrl { url: String },
    /// Archive is newer than the cache
    ArchiveDistNewerThanCache,
    /// The git archive is still path, could be caused by an old source install
    GitArchiveIsPath,
    /// The git commit hash is different from the locked version
    GitCommitsMismatch {
        installed_commit: String,
        locked_commit: String,
    },
    /// Unable to parse the installed git url
    UnableToParseGitUrl { url: String },
    /// Unable to get the installed dist metadata, something is definitely broken
    UnableToGetInstalledDistMetadata,
    /// The requires-python is different than the installed version
    RequiredPythonChanged {
        installed_python_version: uv_pep440::VersionSpecifiers,
        locked_python_version: uv_pep440::Version,
    },
    /// Re-installing because of an installer mismatch, but we are managing the package
    InstallerMismatch { previous_installer: String },
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
            NeedReinstall::EditableStatusChanged { is_now_editable } => {
                write!(
                    f,
                    "Editable status changed, editable status is: {}",
                    is_now_editable
                )
            }
            NeedReinstall::UnableToParseInstalledDistUrl { url } => {
                write!(f, "Unable to parse installed dist url: {}", url)
            }
            NeedReinstall::ArchiveDistNewerThanCache => {
                write!(f, "Archive dist is newer than the cache")
            }
            NeedReinstall::GitArchiveIsPath => write!(f, "Git archive is a path"),
            NeedReinstall::GitCommitsMismatch {
                installed_commit,
                locked_commit,
            } => write!(
                f,
                "Git commits mismatch, installed commit: {}, locked commit: {}",
                installed_commit, locked_commit
            ),
            NeedReinstall::UnableToParseGitUrl { url } => {
                write!(f, "Unable to parse git url: {}", url)
            }
            NeedReinstall::UnableToGetInstalledDistMetadata => {
                write!(f, "Unable to get installed dist metadata")
            }
            NeedReinstall::RequiredPythonChanged {
                installed_python_version,
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
        }
    }
}

enum ValidateCurrentInstall {
    /// Keep this package
    Keep,
    /// Reinstall this package
    Reinstall(NeedReinstall),
}

/// Check if a package needs to be reinstalled
fn need_reinstall(
    installed: &InstalledDist,
    locked: &PypiPackageData,
    python_version: &uv_pep440::Version,
) -> miette::Result<ValidateCurrentInstall> {
    // Check if the installed version is the same as the required version
    match installed {
        InstalledDist::Registry(reg) => {
            let specifier = to_uv_version(&locked.version).into_diagnostic()?;

            if reg.version != specifier {
                return Ok(ValidateCurrentInstall::Reinstall(
                    NeedReinstall::VersionMismatch {
                        installed_version: reg.version.clone(),
                        locked_version: locked.version.clone(),
                    },
                ));
            }
        }

        // For installed distributions check the direct_url.json to check if a re-install is needed
        InstalledDist::Url(direct_url) => {
            let direct_url_json = match InstalledDist::direct_url(&direct_url.path) {
                Ok(Some(direct_url)) => direct_url,
                Ok(None) => {
                    return Ok(ValidateCurrentInstall::Reinstall(
                        NeedReinstall::MissingDirectUrl,
                    ));
                }
                Err(_) => {
                    return Ok(ValidateCurrentInstall::Reinstall(
                        NeedReinstall::MissingDirectUrl,
                    ));
                }
            };

            match direct_url_json {
                uv_pypi_types::DirectUrl::LocalDirectory { url, dir_info } => {
                    // Recreate file url
                    let result = Url::parse(&url);
                    match result {
                        Ok(url) => {
                            // Check if the urls are different
                            if Some(&url) == locked.location.as_url() {
                                // Check cache freshness
                                if !check_url_freshness(&url, installed)? {
                                    return Ok(ValidateCurrentInstall::Reinstall(
                                        NeedReinstall::SourceDirectoryNewerThanCache,
                                    ));
                                }
                            }
                        }
                        Err(_) => {
                            return Ok(ValidateCurrentInstall::Reinstall(
                                NeedReinstall::UnableToParseFileUrl { url },
                            ));
                        }
                    }
                    // If editable status changed also re-install
                    if dir_info.editable.unwrap_or_default() != locked.editable {
                        return Ok(ValidateCurrentInstall::Reinstall(
                            NeedReinstall::EditableStatusChanged {
                                is_now_editable: dir_info.editable.unwrap_or_default(),
                            },
                        ));
                    }
                }
                uv_pypi_types::DirectUrl::ArchiveUrl {
                    url,
                    // Don't think anything ever fills this?
                    archive_info: _,
                    // Subdirectory is either in the url or not supported
                    subdirectory: _,
                } => {
                    let locked_url = match &locked.location {
                        // Remove `direct+` scheme if it is there so we can compare the required to
                        // the installed url
                        UrlOrPath::Url(url) => strip_direct_scheme(url),
                        UrlOrPath::Path(_path) => {
                            return Ok(ValidateCurrentInstall::Reinstall(
                                NeedReinstall::GitArchiveIsPath,
                            ))
                        }
                    };

                    // Try to parse both urls
                    let installed_url = url.parse::<Url>();

                    // Same here
                    let installed_url = if let Ok(installed_url) = installed_url {
                        installed_url
                    } else {
                        return Ok(ValidateCurrentInstall::Reinstall(
                            NeedReinstall::UnableToParseInstalledDistUrl { url },
                        ));
                    };

                    if locked_url.as_ref() == &installed_url {
                        // Check cache freshness
                        if !check_url_freshness(&locked_url, installed)? {
                            return Ok(ValidateCurrentInstall::Reinstall(
                                NeedReinstall::ArchiveDistNewerThanCache,
                            ));
                        }
                    }
                }
                uv_pypi_types::DirectUrl::VcsUrl {
                    url,
                    vcs_info,
                    subdirectory: _,
                } => {
                    let url = Url::parse(&url).into_diagnostic()?;
                    let git_url = match &locked.location {
                        UrlOrPath::Url(url) => ParsedGitUrl::try_from(url.clone()),
                        UrlOrPath::Path(_path) => {
                            // Previously
                            return Ok(ValidateCurrentInstall::Reinstall(
                                NeedReinstall::GitArchiveIsPath,
                            ));
                        }
                    };
                    match git_url {
                        Ok(git) => {
                            // Check the repository base url
                            if git.url.repository() != &url
                                // Check the sha from the direct_url.json and the required sha
                                // Use the uv git url to get the sha
                                || vcs_info.commit_id != git.url.precise().map(|p| p.to_string())
                            {
                                return Ok(ValidateCurrentInstall::Reinstall(
                                    NeedReinstall::GitCommitsMismatch {
                                        installed_commit: vcs_info.commit_id.unwrap_or_default(),
                                        locked_commit: git
                                            .url
                                            .precise()
                                            .map(|p| p.to_string())
                                            .unwrap_or_default(),
                                    },
                                ));
                            }
                        }
                        Err(_) => {
                            return Ok(ValidateCurrentInstall::Reinstall(
                                NeedReinstall::UnableToParseGitUrl {
                                    url: url.to_string(),
                                },
                            ));
                        }
                    }
                }
            }
        }
        // Figure out what to do with these
        InstalledDist::EggInfoFile(installed_egg) => {
            tracing::warn!(
                "egg-info files are not supported yet, skipping: {}",
                installed_egg.name
            );
        }
        InstalledDist::EggInfoDirectory(installed_egg_dir) => {
            tracing::warn!(
                "egg-info directories are not supported yet, skipping: {}",
                installed_egg_dir.name
            );
        }
        InstalledDist::LegacyEditable(egg_link) => {
            tracing::warn!(
                ".egg-link pointers are not supported yet, skipping: {}",
                egg_link.name
            );
        }
    };

    // Do some extra checks if the version is the same
    let metadata = if let Ok(metadata) = installed.metadata() {
        metadata
    } else {
        // Can't be sure lets reinstall
        return Ok(ValidateCurrentInstall::Reinstall(
            NeedReinstall::UnableToGetInstalledDistMetadata,
        ));
    };

    if let Some(requires_python) = metadata.requires_python {
        // If the installed package requires a different python version
        if !requires_python.contains(python_version) {
            return Ok(ValidateCurrentInstall::Reinstall(
                NeedReinstall::RequiredPythonChanged {
                    installed_python_version: requires_python,
                    locked_python_version: python_version.clone(),
                },
            ));
        }
    }

    Ok(ValidateCurrentInstall::Keep)
}

// Below we define a couple of traits so that we can make the creaton of the install plan
// somewhat more abstract

/// Provide an iterator over the installed distributions
/// This trait can also be used to mock the installed distributions for testing purposes
pub trait InstalledDistProvider<'a> {
    /// Provide an iterator over the installed distributions
    fn iter(&'a self) -> impl Iterator<Item = &'a InstalledDist>;
}

impl<'a> InstalledDistProvider<'a> for SitePackages {
    fn iter(&'a self) -> impl Iterator<Item = &'a InstalledDist> {
        self.iter()
    }
}

/// Provides a way to get the potentially cached distribution, if it exists
/// This trait can also be used to mock the cache for testing purposes
pub trait CachedDistProvider<'a> {
    /// Get the cached distribution for a package name and version
    fn get_cached_dist(
        &mut self,
        name: &'a uv_normalize::PackageName,
        version: uv_pep440::Version,
    ) -> Option<CachedRegistryDist>;
}

impl<'a> CachedDistProvider<'a> for RegistryWheelIndex<'a> {
    fn get_cached_dist(
        &mut self,
        name: &'a uv_normalize::PackageName,
        version: uv_pep440::Version,
    ) -> Option<CachedRegistryDist> {
        let index = self
            .get(name)
            .find(|entry| entry.dist.filename.version == version);
        index.map(|index| index.dist.clone())
    }
}

/// Struct that handles the planning of the installation
/// of the PyPI packages into an existing conda environment with specific
/// locked data
///
/// When executing the [`InstallPlanner::plan`] method, we will figure out what
/// we can link from the cache locally and what we need to download from the registry.
/// As well as determine what we need to remove, which we call extraneous packages.
///
/// This is all inspired by the structs and methods in the uv crate, specifically the `uv_installer` module.
/// But all of it is heavily modified as we need to use our locked data for comparison, and also ignore some things
/// that uv would usually act on.
pub struct InstallPlanner {
    uv_cache: Cache,
    python_version: uv_pep440::Version,
    lock_file_dir: PathBuf,
}

impl InstallPlanner {
    pub fn new(
        uv_cache: Cache,
        python_version: &uv_pep440::Version,
        lock_file_dir: impl AsRef<Path>,
    ) -> Self {
        Self {
            uv_cache,
            python_version: python_version.clone(),
            lock_file_dir: lock_file_dir.as_ref().to_path_buf(),
        }
    }

    /// Decide if we need to get the distribution from the local cache or the registry
    /// this method will add the distribution to the local or remote vector,
    /// depending on whether the version is stale, available locally or not
    fn decide_installation_source<'a, Op: OperationToReason>(
        &self,
        name: &'a uv_normalize::PackageName,
        required_pkg: &PypiPackageData,
        local: &mut Vec<(CachedDist, InstallReason)>,
        remote: &mut Vec<(Dist, InstallReason)>,
        dist_cache: &mut impl CachedDistProvider<'a>,
        op_to_reason: Op,
    ) -> miette::Result<()> {
        // Okay so we need to re-install the package
        // let's see if we need the remote or local version

        // First, check if we need to revalidate the package
        // then we should get it from the remote
        if self.uv_cache.must_revalidate(name) {
            remote.push((
                convert_to_dist(required_pkg, &self.lock_file_dir).into_diagnostic()?,
                op_to_reason.stale(),
            ));
            return Ok(());
        }
        let uv_version = to_uv_version(&required_pkg.version).into_diagnostic()?;
        // If it is not stale its either in the registry cache or not
        let cached = dist_cache.get_cached_dist(name, uv_version);
        // If we have it in the cache we can use that
        if let Some(distribution) = cached {
            local.push((CachedDist::Registry(distribution), op_to_reason.cached()));
        // If we don't have it in the cache we need to download it
        } else {
            remote.push((
                convert_to_dist(required_pkg, &self.lock_file_dir).into_diagnostic()?,
                op_to_reason.missing(),
            ));
        }
        Ok(())
    }

    /// Figure out what we can link from the cache locally
    /// and what we need to download from the registry.
    /// Also determine what we need to remove.
    ///
    /// All the 'a lifetimes are to to make sure that the names provided to the CachedDistProvider
    /// are valid for the lifetime of the CachedDistProvider and what is passed to the method
    pub fn plan<'a, Installed: InstalledDistProvider<'a>, Cached: CachedDistProvider<'a> + 'a>(
        &self,
        site_packages: &'a Installed,
        mut dist_cache: Cached,
        required_pkgs: &'a HashMap<uv_normalize::PackageName, &PypiPackageData>,
    ) -> miette::Result<PixiInstallPlan> {
        // Packages to be removed
        let mut extraneous = vec![];
        // Packages to be installed directly from the cache
        let mut local = vec![];
        // Try to install from the registry or direct url or w/e
        let mut remote = vec![];
        // Packages that need to be reinstalled
        // i.e. need to be removed before being installed
        let mut reinstalls = vec![];

        // Will contain the packages that have been previously installed
        // and a decision has been made what to do with them
        let mut prev_installed_packages = HashSet::new();

        // Walk over all installed packages and check if they are required
        for dist in site_packages.iter() {
            // Check if we require the package to be installed
            let pkg = required_pkgs.get(dist.name());
            // Get the installer name
            let installer = dist
                .installer()
                // Empty string if no installer or any other error
                .map_or(String::new(), |f| f.unwrap_or_default());

            match pkg {
                Some(required_pkg) => {
                    // Add to the list of previously installed packages
                    prev_installed_packages.insert(dist.name());
                    // Check if we need this package installed but it is not currently installed by us
                    if installer != consts::PIXI_UV_INSTALLER {
                        // We are managing the package but something else has installed a version
                        // let's re-install to make sure that we have the **correct** version
                        reinstalls.push((
                            dist.clone(),
                            NeedReinstall::InstallerMismatch {
                                previous_installer: installer.clone(),
                            },
                        ));
                    } else {
                        // Check if we need to reinstall
                        match need_reinstall(dist, required_pkg, &self.python_version)? {
                            ValidateCurrentInstall::Keep => {
                                // No need to reinstall
                                continue;
                            }
                            ValidateCurrentInstall::Reinstall(reason) => {
                                reinstalls.push((dist.clone(), reason));
                            }
                        }
                    }
                    // Okay so we need to re-install the package
                    // let's see if we need the remote or local version
                    self.decide_installation_source(
                        dist.name(),
                        required_pkg,
                        &mut local,
                        &mut remote,
                        &mut dist_cache,
                        Reinstall,
                    )?;
                }
                // Second case we are not managing the package
                None if installer != consts::PIXI_UV_INSTALLER => {
                    // Ignore packages that we are not managed by us
                    continue;
                }
                // Third case we *are* managing the package but it is no longer required
                None => {
                    // Add to the extraneous list
                    // as we do manage it but have no need for it
                    extraneous.push(dist.clone());
                }
            }
        }

        // Now we need to check if we have any packages left in the required_map
        for (name, pkg) in required_pkgs
            .iter()
            // Only check the packages that have not been previously installed
            .filter(|(name, _)| !prev_installed_packages.contains(name))
        {
            // Decide if we need to get the distribution from the local cache or the registry
            self.decide_installation_source(
                name,
                pkg,
                &mut local,
                &mut remote,
                &mut dist_cache,
                Install,
            )?;
        }

        Ok(PixiInstallPlan {
            local,
            remote,
            reinstalls,
            extraneous,
        })
    }
}
