use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fs,
    path::Path,
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use itertools::Itertools;
use miette::{IntoDiagnostic, WrapErr};
use pep440_rs::{Version, VersionSpecifiers};
use pixi_consts::consts;
use pixi_manifest::{pyproject::PyProjectManifest, SystemRequirements};
use pixi_record::PixiRecord;
use pixi_uv_conversions::{
    isolated_names_to_packages, locked_indexes_to_index_locations, names_to_build_isolation,
    to_uv_normalize, to_uv_version, to_uv_version_specifiers, ConversionError,
};
use pypi_modifiers::pypi_tags::{get_pypi_tags, is_python_record};
use rattler_conda_types::Platform;
use rattler_lock::{
    PackageHashes, PypiIndexes, PypiPackageData, PypiPackageEnvironmentData, UrlOrPath,
};
use url::Url;
use uv_auth::store_credentials_from_url;
use uv_cache::{ArchiveTarget, ArchiveTimestamp, Cache};
use uv_client::{Connectivity, FlatIndexClient, RegistryClientBuilder};
use uv_configuration::{ConfigSettings, Constraints, IndexStrategy, LowerBound};
use uv_dispatch::BuildDispatch;
use uv_distribution::{DistributionDatabase, RegistryWheelIndex};
use uv_distribution_filename::{DistExtension, ExtensionError, SourceDistExtension, WheelFilename};
use uv_distribution_types::{
    BuiltDist, CachedDist, DependencyMetadata, Dist, IndexLocations, IndexUrl, InstalledDist, Name,
    RegistryBuiltDist, RegistryBuiltWheel, RegistrySourceDist, SourceDist, UrlString,
};
use uv_git::GitResolver;
use uv_install_wheel::linker::LinkMode;
use uv_installer::{Preparer, SitePackages, UninstallError};
use uv_pep508::{VerbatimUrl, VerbatimUrlError};
use uv_pypi_types::{
    HashAlgorithm, HashDigest, ParsedGitUrl, ParsedUrl, ParsedUrlError, VerbatimParsedUrl,
};
use uv_python::{Interpreter, PythonEnvironment};
use uv_resolver::{FlatIndex, InMemoryIndex};
use uv_types::HashStrategy;

use crate::{
    conda_pypi_clobber::PypiCondaClobberRegistry,
    lock_file::UvResolutionContext,
    prefix::Prefix,
    uv_reporter::{UvReporter, UvReporterOptions},
};

type CombinedPypiPackageData = (PypiPackageData, PypiPackageEnvironmentData);

fn elapsed(duration: Duration) -> String {
    let secs = duration.as_secs();

    if secs >= 60 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else if secs > 0 {
        format!("{}.{:02}s", secs, duration.subsec_nanos() / 10_000_000)
    } else {
        format!("{}ms", duration.subsec_millis())
    }
}

#[derive(Debug)]
enum InstallReason {
    ReinstallCached,
    ReinstallStaleLocal,
    ReinstallMissing,
    InstallStaleLocal,
    InstallMissing,
    InstallCached,
}

/// Derived from uv [`uv_installer::Plan`]
#[derive(Debug)]
struct PixiInstallPlan {
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

/// Converts our locked data to a file
fn locked_data_to_file(
    url: &Url,
    hash: Option<&PackageHashes>,
    filename: &str,
    requires_python: Option<VersionSpecifiers>,
) -> Result<uv_distribution_types::File, ConversionError> {
    let url = uv_distribution_types::FileLocation::AbsoluteUrl(UrlString::from(url.clone()));

    // Convert PackageHashes to uv hashes
    let hashes = if let Some(hash) = hash {
        match hash {
            rattler_lock::PackageHashes::Md5(md5) => vec![HashDigest {
                algorithm: HashAlgorithm::Md5,
                digest: format!("{:x}", md5).into(),
            }],
            rattler_lock::PackageHashes::Sha256(sha256) => vec![HashDigest {
                algorithm: HashAlgorithm::Sha256,
                digest: format!("{:x}", sha256).into(),
            }],
            rattler_lock::PackageHashes::Md5Sha256(md5, sha256) => vec![
                HashDigest {
                    algorithm: HashAlgorithm::Md5,
                    digest: format!("{:x}", md5).into(),
                },
                HashDigest {
                    algorithm: HashAlgorithm::Sha256,
                    digest: format!("{:x}", sha256).into(),
                },
            ],
        }
    } else {
        vec![]
    };

    let uv_requires_python = requires_python
        .map(|inside| to_uv_version_specifiers(&inside))
        .transpose()?;

    Ok(uv_distribution_types::File {
        filename: filename.to_string(),
        dist_info_metadata: false,
        hashes,
        requires_python: uv_requires_python,
        upload_time_utc_ms: None,
        yanked: None,
        size: None,
        url,
    })
}

/// Check if the url is a direct url
/// Files, git, are direct urls
/// Direct urls to wheels or sdists are prefixed with a `direct` scheme
/// by us when resolving the lock file
fn is_direct_url(url_scheme: &str) -> bool {
    url_scheme == "file"
        || url_scheme == "git+http"
        || url_scheme == "git+https"
        || url_scheme == "git+ssh"
        || url_scheme.starts_with("direct")
}

/// Strip of the `direct` scheme from the url if it is there
fn strip_direct_scheme(url: &Url) -> Cow<'_, Url> {
    url.as_ref()
        .strip_prefix("direct+")
        .and_then(|str| Url::from_str(str).ok())
        .map(Cow::Owned)
        .unwrap_or(Cow::Borrowed(url))
}

#[derive(Debug, thiserror::Error)]
enum ConvertToUvDistError {
    #[error("error creating ParsedUrl")]
    ParseUrl(#[from] Box<ParsedUrlError>),
    #[error("error creating uv Dist from url")]
    Uv(#[from] uv_distribution_types::Error),
    #[error("error constructing verbatim url")]
    VerbatimUrl(#[from] VerbatimUrlError),
    #[error("error extracting extension from {1}")]
    Extension(#[source] ExtensionError, String),

    #[error(transparent)]
    UvPepTypes(#[from] ConversionError),
}

/// Convert from a PypiPackageData to a uv [`distribution_types::Dist`]
fn convert_to_dist(
    pkg: &PypiPackageData,
    lock_file_dir: &Path,
) -> Result<Dist, ConvertToUvDistError> {
    // Figure out if it is a url from the registry or a direct url
    let dist = match &pkg.location {
        UrlOrPath::Url(url) if is_direct_url(url.scheme()) => {
            let url_without_direct = strip_direct_scheme(url);
            let pkg_name = to_uv_normalize(&pkg.name)?;
            Dist::from_url(
                pkg_name,
                VerbatimParsedUrl {
                    parsed_url: ParsedUrl::try_from(url_without_direct.clone().into_owned())
                        .map_err(Box::new)?,
                    verbatim: VerbatimUrl::from(url_without_direct.into_owned()),
                },
            )?
        }
        UrlOrPath::Url(url) => {
            // We consider it to be a registry url
            // Extract last component from registry url
            // should be something like `package-0.1.0-py3-none-any.whl`
            let filename_raw = url.path_segments().unwrap().last().unwrap();

            // Decode the filename to avoid issues with the HTTP coding like `%2B` to `+`
            let filename_decoded =
                percent_encoding::percent_decode_str(filename_raw).decode_utf8_lossy();

            // Now we can convert the locked data to a [`distribution_types::File`]
            // which is essentially the file information for a wheel or sdist
            let file = locked_data_to_file(
                url,
                pkg.hash.as_ref(),
                filename_decoded.as_ref(),
                pkg.requires_python.clone(),
            )?;

            // Recreate the filename from the extracted last component
            // If this errors this is not a valid wheel filename
            // and we should consider it a sdist
            let filename = WheelFilename::from_str(filename_decoded.as_ref());
            if let Ok(filename) = filename {
                Dist::Built(BuiltDist::Registry(RegistryBuiltDist {
                    wheels: vec![RegistryBuiltWheel {
                        filename,
                        file: Box::new(file),
                        // This should be fine because currently it is only used for caching
                        // When upgrading uv and running into problems we would need to sort this
                        // out but it would require adding the indexes to
                        // the lock file
                        index: IndexUrl::Pypi(VerbatimUrl::from_url(
                            consts::DEFAULT_PYPI_INDEX_URL.clone(),
                        )),
                    }],
                    best_wheel_index: 0,
                    sdist: None,
                }))
            } else {
                let pkg_name = to_uv_normalize(&pkg.name)?;
                let pkg_version = to_uv_version(&pkg.version)?;
                Dist::Source(SourceDist::Registry(RegistrySourceDist {
                    name: pkg_name,
                    version: pkg_version,
                    file: Box::new(file),
                    // This should be fine because currently it is only used for caching
                    index: IndexUrl::Pypi(VerbatimUrl::from_url(
                        consts::DEFAULT_PYPI_INDEX_URL.clone(),
                    )),
                    // I don't think this really matters for the install
                    wheels: vec![],
                    ext: SourceDistExtension::from_path(Path::new(filename_raw)).map_err(|e| {
                        ConvertToUvDistError::Extension(e, filename_raw.to_string())
                    })?,
                }))
            }
        }
        UrlOrPath::Path(path) => {
            let native_path = Path::new(path.as_str());
            let abs_path = if path.is_absolute() {
                native_path.to_path_buf()
            } else {
                lock_file_dir.join(native_path)
            };

            let absolute_url = VerbatimUrl::from_absolute_path(&abs_path)?;
            let pkg_name =
                uv_normalize::PackageName::new(pkg.name.to_string()).expect("should be correct");
            if abs_path.is_dir() {
                Dist::from_directory_url(pkg_name, absolute_url, &abs_path, pkg.editable, false)?
            } else {
                Dist::from_file_url(
                    pkg_name,
                    absolute_url,
                    &abs_path,
                    DistExtension::from_path(&abs_path).map_err(|e| {
                        ConvertToUvDistError::Extension(e, abs_path.to_string_lossy().to_string())
                    })?,
                )?
            }
        }
    };

    Ok(dist)
}

/// Check freshness of a locked url against an installed dist
fn check_url_freshness(locked_url: &Url, installed_dist: &InstalledDist) -> miette::Result<bool> {
    if let Ok(archive) = locked_url.to_file_path() {
        // This checks the entrypoints like `pyproject.toml`, `setup.cfg`, and
        // `setup.py` against the METADATA of the installed distribution
        if ArchiveTimestamp::up_to_date_with(&archive, ArchiveTarget::Install(installed_dist))
            .into_diagnostic()?
        {
            tracing::debug!("Requirement already satisfied (and up-to-date): {installed_dist}");
            Ok(true)
        } else {
            tracing::debug!("Requirement already satisfied (but not up-to-date): {installed_dist}");
            Ok(false)
        }
    } else {
        // Otherwise, assume the requirement is up-to-date.
        tracing::debug!("Requirement already satisfied (assumed up-to-date): {installed_dist}");
        Ok(true)
    }
}

/// Represents the different reasons why a package needs to be reinstalled
#[derive(Debug)]
pub(crate) enum NeedReinstall {
    /// The package is not installed
    VersionMismatch {
        installed_version: uv_pep440::Version,
        locked_version: Version,
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
    python_version: &Version,
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
        let uv_version = to_uv_version(python_version).into_diagnostic()?;
        if !requires_python.contains(&uv_version) {
            return Ok(ValidateCurrentInstall::Reinstall(
                NeedReinstall::RequiredPythonChanged {
                    installed_python_version: requires_python,
                    locked_python_version: uv_version,
                },
            ));
        }
    }

    Ok(ValidateCurrentInstall::Keep)
}

/// Figure out what we can link from the cache locally
/// and what we need to download from the registry.
/// Also determine what we need to remove.
fn whats_the_plan<'a>(
    site_packages: &'a mut SitePackages,
    mut registry_index: RegistryWheelIndex<'a>,
    required_pkgs: &'a HashMap<uv_normalize::PackageName, &'a PypiPackageData>,
    uv_cache: &Cache,
    python_version: &Version,
    lock_file_dir: &Path,
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
            Some(pkg) => {
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
                    match need_reinstall(dist, pkg, python_version)? {
                        ValidateCurrentInstall::Keep => {
                            // No need to reinstall
                            // Remove from the required map
                            continue;
                        }
                        ValidateCurrentInstall::Reinstall(reason) => {
                            reinstalls.push((dist.clone(), reason));
                        }
                    }
                }

                // Okay so we need to re-install the package
                // let's see if we need the remote or local version

                // First, check if we need to revalidate the package
                // then we should get it from the remote
                if uv_cache.must_revalidate(dist.name()) {
                    remote.push((
                        convert_to_dist(pkg, lock_file_dir).into_diagnostic()?,
                        InstallReason::ReinstallStaleLocal,
                    ));
                    continue;
                }
                let uv_version = to_uv_version(&pkg.version).into_diagnostic()?;
                // If it is not stale its either in the registry cache or not
                let wheel = registry_index
                    .get(dist.name())
                    .find(|entry| entry.dist.filename.version == uv_version);

                // If we have it in the cache we can use that
                if let Some(cached) = wheel {
                    let entire_cloned = cached.clone();
                    local.push((
                        CachedDist::Registry(entire_cloned.dist.clone()),
                        InstallReason::ReinstallCached,
                    ));
                // If we don't have it in the cache we need to download it
                } else {
                    remote.push((
                        convert_to_dist(pkg, lock_file_dir).into_diagnostic()?,
                        InstallReason::ReinstallMissing,
                    ));
                }
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
        // Check if we need to revalidate
        // In that case we need to download from the registry
        if uv_cache.must_revalidate(name) {
            remote.push((
                convert_to_dist(pkg, lock_file_dir).into_diagnostic()?,
                InstallReason::InstallStaleLocal,
            ));
            continue;
        }

        let uv_version = to_uv_version(&pkg.version).into_diagnostic()?;

        // Do we have in the registry cache?
        let wheel = registry_index
            .get(name)
            .find(|entry| entry.dist.filename.version == uv_version)
            .cloned();
        if let Some(cached) = wheel {
            // Sure we have it in the cache, lets use that
            local.push((
                CachedDist::Registry(cached.dist),
                InstallReason::InstallCached,
            ));
        } else {
            // We need to download from the registry or any url
            remote.push((
                convert_to_dist(pkg, lock_file_dir).into_diagnostic()?,
                InstallReason::InstallMissing,
            ));
        }
    }

    Ok(PixiInstallPlan {
        local,
        remote,
        reinstalls,
        extraneous,
    })
}

/// Installs and/or remove python distributions.
// TODO: refactor arguments in struct
#[allow(clippy::too_many_arguments)]
pub async fn update_python_distributions(
    lock_file_dir: &Path,
    prefix: &Prefix,
    pixi_records: &[PixiRecord],
    python_packages: &[CombinedPypiPackageData],
    python_interpreter_path: &Path,
    system_requirements: &SystemRequirements,
    uv_context: &UvResolutionContext,
    pypi_indexes: Option<&PypiIndexes>,
    environment_variables: &HashMap<String, String>,
    platform: Platform,
    non_isolated_packages: Option<Vec<String>>,
) -> miette::Result<()> {
    let start = std::time::Instant::now();

    // Determine the current environment markers.
    let python_record = pixi_records
        .iter()
        .find(|r| is_python_record(r))
        .ok_or_else(|| miette::miette!("could not resolve pypi dependencies because no python interpreter is added to the dependencies of the project.\nMake sure to add a python interpreter to the [dependencies] section of the {manifest}, or run:\n\n\tpixi add python", manifest=consts::PROJECT_MANIFEST))?;
    let tags = get_pypi_tags(
        platform,
        system_requirements,
        python_record.package_record(),
    )?;

    let index_locations = pypi_indexes
        .map(|indexes| locked_indexes_to_index_locations(indexes, lock_file_dir))
        .unwrap_or_else(|| Ok(IndexLocations::default()))
        .into_diagnostic()?;

    let registry_client = Arc::new(
        RegistryClientBuilder::new(uv_context.cache.clone())
            .client(uv_context.client.clone())
            .index_urls(index_locations.index_urls())
            .keyring(uv_context.keyring_provider)
            .connectivity(Connectivity::Online)
            .build(),
    );

    // Resolve the flat indexes from `--find-links`.
    let flat_index = {
        let client = FlatIndexClient::new(&registry_client, &uv_context.cache);
        let indexes = index_locations.flat_indexes().map(|index| index.url());
        let entries = client.fetch(indexes).await.into_diagnostic()?;
        FlatIndex::from_entries(
            entries,
            Some(&tags),
            &uv_types::HashStrategy::None,
            &uv_context.build_options,
        )
    };

    let in_memory_index = InMemoryIndex::default();
    let config_settings = ConfigSettings::default();

    // Setup the interpreter from the conda prefix
    let python_location = prefix.root().join(python_interpreter_path);
    let interpreter = Interpreter::query(&python_location, &uv_context.cache).into_diagnostic()?;
    tracing::debug!(
        "installing with python interpreter: {} from {}",
        interpreter.key(),
        interpreter.sys_prefix().display()
    );

    // Create a Python environment
    let venv = PythonEnvironment::from_interpreter(interpreter);
    let non_isolated_packages =
        isolated_names_to_packages(non_isolated_packages.as_deref()).into_diagnostic()?;
    // Determine if we need to build any packages in isolation
    let build_isolation = names_to_build_isolation(non_isolated_packages.as_deref(), &venv);

    let git_resolver = GitResolver::default();

    let dep_metadata = DependencyMetadata::default();
    let constraints = Constraints::default();
    let build_dispatch = BuildDispatch::new(
        &registry_client,
        &uv_context.cache,
        constraints,
        venv.interpreter(),
        &index_locations,
        &flat_index,
        &dep_metadata,
        &in_memory_index,
        &git_resolver,
        &uv_context.capabilities,
        &uv_context.in_flight,
        IndexStrategy::default(),
        &config_settings,
        build_isolation,
        LinkMode::default(),
        &uv_context.build_options,
        &uv_context.hash_strategy,
        None,
        LowerBound::default(),
        uv_context.source_strategy,
        uv_context.concurrency,
    )
    // ! Important this passes any CONDA activation to the uv build process
    .with_build_extra_env_vars(environment_variables.iter());

    let _lock = venv
        .lock()
        .await
        .into_diagnostic()
        .with_context(|| "error locking installation directory")?;

    // Find out what packages are already installed
    let mut site_packages =
        SitePackages::from_environment(&venv).expect("could not create site-packages");

    tracing::debug!(
        "Constructed site-packages with {} packages",
        site_packages.iter().count(),
    );

    // This is used to find wheels that are available from the registry
    let registry_index = RegistryWheelIndex::new(
        &uv_context.cache,
        &tags,
        &index_locations,
        &HashStrategy::None,
    );

    // Create a map of the required packages
    let required_map: std::collections::HashMap<uv_normalize::PackageName, &PypiPackageData> =
        python_packages
            .iter()
            .map(|(pkg, _)| {
                let uv_name = uv_normalize::PackageName::new(pkg.name.to_string())
                    .expect("should be correct");
                (uv_name, pkg)
            })
            .collect();

    // Partition into those that should be linked from the cache (`local`), those
    // that need to be downloaded (`remote`), and those that should be removed
    // (`extraneous`).
    let PixiInstallPlan {
        local,
        remote,
        reinstalls,
        extraneous,
    } = whats_the_plan(
        &mut site_packages,
        registry_index,
        &required_map,
        &uv_context.cache,
        &pep440_rs::Version::from_str(&venv.interpreter().python_version().to_string())
            .expect("should be the same"),
        lock_file_dir,
    )?;

    // Determine the currently installed conda packages.
    let installed_packages = prefix
        .find_installed_packages(None)
        .await
        .with_context(|| {
            format!(
                "failed to determine the currently installed packages for {}",
                prefix.root().display()
            )
        })?;

    let pypi_conda_clobber = PypiCondaClobberRegistry::with_conda_packages(&installed_packages);

    // Show totals
    let total_to_install = local.len() + remote.len();
    let total_required = required_map.len();
    tracing::info!(
        "{} of {} required packages are considered are installed and up-to-date",
        total_required - total_to_install,
        total_required
    );
    // Nothing to do.
    if remote.is_empty() && local.is_empty() && reinstalls.is_empty() && extraneous.is_empty() {
        tracing::info!(
            "{}",
            format!("Nothing to do - finished in {}", elapsed(start.elapsed()))
        );
        return Ok(());
    }

    // Installation and re-installation have needed a lot of debugging in the past
    // That's why we do a bit more extensive logging here
    // This is a bit verbose but it is very helpful when debugging
    // Not enable `-vv` to get the full debug output for reinstallation
    let mut install_cached = vec![];
    let mut install_stale = vec![];
    let mut install_missing = vec![];

    // Filter out the re-installs, mostly these are less interesting than the actual re-install reasons
    // do show the installs
    for (dist, reason) in local.iter() {
        match reason {
            InstallReason::InstallStaleLocal => install_stale.push(dist.name().to_string()),
            InstallReason::InstallMissing => install_missing.push(dist.name().to_string()),
            InstallReason::InstallCached => install_cached.push(dist.name().to_string()),
            _ => {}
        }
    }
    for (dist, reason) in remote.iter() {
        match reason {
            InstallReason::InstallStaleLocal => install_stale.push(dist.name().to_string()),
            InstallReason::InstallMissing => install_missing.push(dist.name().to_string()),
            InstallReason::InstallCached => install_cached.push(dist.name().to_string()),
            _ => {}
        }
    }

    if !install_missing.is_empty() {
        tracing::info!(
            "*installing* from remote because no version is cached: {}",
            install_missing.iter().join(", ")
        );
    }
    if !install_stale.is_empty() {
        tracing::info!(
            "*installing* from remote because local version is stale: {}",
            install_stale.iter().join(", ")
        );
    }
    if !install_cached.is_empty() {
        tracing::info!(
            "*installing* cached version because cache is up-to-date: {}",
            install_cached.iter().join(", ")
        );
    }

    if !reinstalls.is_empty() {
        tracing::info!(
            "*re-installing* following packages: {}",
            reinstalls.iter().map(|(d, _)| d.name()).join(", ")
        );
        // List all re-install reasons
        for (dist, reason) in &reinstalls {
            // Only log the re-install reason if it is not an installer mismatch
            if !matches!(reason, NeedReinstall::InstallerMismatch { .. }) {
                tracing::debug!("re-installing {} because: {}", dist.name(), reason);
            }
        }
    }
    if !extraneous.is_empty() {
        // List all packages that will be removed
        tracing::info!(
            "*removing* following packages: {}",
            extraneous.iter().map(|d| d.name()).join(", ")
        );
    }

    // Download, build, and unzip any missing distributions.
    let wheels = if remote.is_empty() {
        Vec::new()
    } else {
        let start = std::time::Instant::now();

        let options = UvReporterOptions::new()
            .with_length(remote.len() as u64)
            .with_capacity(remote.len() + 30)
            .with_starting_tasks(remote.iter().map(|(d, _)| format!("{}", d.name())))
            .with_top_level_message("Preparing distributions");

        let distribution_database = DistributionDatabase::new(
            registry_client.as_ref(),
            &build_dispatch,
            uv_context.concurrency.downloads,
        );

        // Before hitting the network let's make sure the credentials are available to
        // uv
        for url in index_locations.indexes().map(|index| index.url()) {
            let success = store_credentials_from_url(url);
            tracing::debug!("Stored credentials for {}: {}", url, success);
        }

        let preparer = Preparer::new(
            &uv_context.cache,
            &tags,
            &uv_types::HashStrategy::None,
            &uv_context.build_options,
            distribution_database,
        )
        .with_reporter(UvReporter::new(options));

        let wheels = preparer
            .prepare(
                remote.iter().map(|(d, _)| d.clone()).collect(),
                &uv_context.in_flight,
            )
            .await
            .into_diagnostic()
            .context("Failed to prepare distributions")?;

        let s = if wheels.len() == 1 { "" } else { "s" };
        tracing::info!(
            "{}",
            format!(
                "Prepared {} in {}",
                format!("{} package{}", wheels.len(), s),
                elapsed(start.elapsed())
            )
        );

        wheels
    };

    // Remove any unnecessary packages.
    if !extraneous.is_empty() || !reinstalls.is_empty() {
        let start = std::time::Instant::now();

        for dist_info in extraneous.iter().chain(reinstalls.iter().map(|(d, _)| d)) {
            let summary = match uv_installer::uninstall(dist_info).await {
                Ok(sum) => sum,
                // Get error types from uv_installer
                Err(UninstallError::Uninstall(e))
                    if matches!(e, uv_install_wheel::Error::MissingRecord(_))
                        || matches!(e, uv_install_wheel::Error::MissingTopLevel(_)) =>
                {
                    // If the uninstallation failed, remove the directory manually and continue
                    tracing::debug!("Uninstall failed for {:?} with error: {}", dist_info, e);

                    // Sanity check to avoid calling remove all on a bad path.
                    if dist_info
                        .path()
                        .iter()
                        .any(|segment| Path::new(segment) == Path::new("site-packages"))
                    {
                        tokio::fs::remove_dir_all(dist_info.path())
                            .await
                            .into_diagnostic()?;
                    }

                    continue;
                }
                Err(err) => {
                    return Err(miette::miette!(err));
                }
            };
            tracing::debug!(
                "Uninstalled {} ({} file{}, {} director{})",
                dist_info.name(),
                summary.file_count,
                if summary.file_count == 1 { "" } else { "s" },
                summary.dir_count,
                if summary.dir_count == 1 { "y" } else { "ies" },
            );
        }

        let s = if extraneous.len() + reinstalls.len() == 1 {
            ""
        } else {
            "s"
        };
        tracing::debug!(
            "{}",
            format!(
                "Uninstalled {} in {}",
                format!("{} package{}", extraneous.len() + reinstalls.len(), s),
                elapsed(start.elapsed())
            )
        );
    }

    // Install the resolved distributions.
    let local_iter = local.iter().map(|(d, _)| d.clone());
    let wheels = wheels.into_iter().chain(local_iter).collect::<Vec<_>>();

    // Figure what wheels needed to be re-installed because of an installer mismatch
    // we want to handle these somewhat differently and warn the user about them
    let mut installer_mismatch = reinstalls
        .iter()
        .filter_map(|(d, reason)| {
            if matches!(reason, NeedReinstall::InstallerMismatch { .. }) {
                Some(d.name().to_string())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    // Verify if pypi wheels will override existing conda packages
    // and warn if they are
    if let Ok(Some(clobber_packages)) =
        pypi_conda_clobber.clobber_on_installation(wheels.clone(), &venv)
    {
        let packages_names = clobber_packages.iter().join(", ");

        tracing::warn!("These conda-packages will be overridden by pypi: \n\t{packages_names}");

        // because we are removing conda packages
        // we filter the ones we already warn
        if !installer_mismatch.is_empty() {
            installer_mismatch.retain(|name| !packages_names.contains(name));
        }
    }

    if !installer_mismatch.is_empty() {
        // Notify the user if there are any packages that were re-installed because they
        // were installed by a different installer.
        let packages = installer_mismatch
            .iter()
            .map(|name| name.to_string())
            .join(", ");
        // BREAK(0.20.1): change this into a warning in a future release
        tracing::info!("These pypi-packages were re-installed because they were previously installed by a different installer but are currently managed by pixi: {packages}")
    }

    let options = UvReporterOptions::new()
        .with_length(wheels.len() as u64)
        .with_capacity(wheels.len() + 30)
        .with_starting_tasks(wheels.iter().map(|d| format!("{}", d.name())))
        .with_top_level_message("Installing distributions");

    if !wheels.is_empty() {
        let start = std::time::Instant::now();
        uv_installer::Installer::new(&venv)
            .with_link_mode(LinkMode::default())
            .with_installer_name(Some(consts::PIXI_UV_INSTALLER.to_string()))
            .with_reporter(UvReporter::new(options))
            .install(wheels.clone())
            .await
            .unwrap();

        let s = if wheels.len() == 1 { "" } else { "s" };
        tracing::info!(
            "{}",
            format!(
                "Installed {} in {}",
                format!("{} package{}", wheels.len(), s),
                elapsed(start.elapsed())
            )
        );
    }

    Ok(())
}

/// Returns `true` if the source tree at the given path contains dynamic
/// metadata.
#[allow(dead_code)]
fn is_dynamic(path: &Path) -> bool {
    // return true;
    // If there's no `pyproject.toml`, we assume it's dynamic.
    let Ok(contents) = fs::read_to_string(path.join("pyproject.toml")) else {
        return true;
    };
    let Ok(pyproject_toml) = PyProjectManifest::from_toml_str(&contents) else {
        return true;
    };
    // // If `[project]` is not present, we assume it's dynamic.
    let Some(project) = pyproject_toml.project() else {
        // ...unless it appears to be a Poetry project.
        return pyproject_toml.poetry().is_none();
    };
    // `[project.dynamic]` must be present and non-empty.
    project
        .dynamic
        .as_ref()
        .is_some_and(|dynamic| !dynamic.is_empty())
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, str::FromStr};

    use pep440_rs::Version;
    use rattler_lock::{PypiPackageData, UrlOrPath};
    use uv_distribution_types::RemoteSource;

    use super::convert_to_dist;

    #[test]
    /// Create locked pypi data, pass this into the convert_to_dist function
    fn convert_special_chars_wheelname_to_dist() {
        // Create url with special characters
        let wheel = "torch-2.3.0%2Bcu121-cp312-cp312-win_amd64.whl";
        let url = format!("https://example.com/{}", wheel).parse().unwrap();
        // Pass into locked data
        let locked = PypiPackageData {
            name: "torch".parse().unwrap(),
            version: Version::from_str("2.3.0+cu121").unwrap(),
            location: UrlOrPath::Url(url),
            hash: None,
            requires_dist: vec![],
            requires_python: None,
            editable: false,
        };

        // Convert the locked data to a uv dist
        // check if it does not panic
        let dist = convert_to_dist(&locked, &PathBuf::new())
            .expect("could not convert wheel with special chars to dist");

        // Check if the dist is a built dist
        assert!(!dist.filename().unwrap().contains("%2B"));
    }
}
