use std::path::{Path, PathBuf};

use pixi_git::url::RepositoryUrl;
use pixi_record::LockedGitUrl;
use pixi_uv_conversions::{to_parsed_git_url, to_uv_version};
use rattler_lock::{PypiPackageData, UrlOrPath};
use url::Url;
use uv_distribution_types::InstalledDist;
use uv_pypi_types::{ParsedGitUrl, ParsedUrlError};

use crate::install_pypi::utils::{check_url_freshness, strip_direct_scheme};

use super::{NeedReinstall, models::ValidateCurrentInstall};
use pixi_uv_conversions::ConversionError;

#[derive(thiserror::Error, Debug)]
pub enum NeedsReinstallError {
    #[error(transparent)]
    UvConversion(#[from] ConversionError),
    #[error(transparent)]
    Conversion(#[from] url::ParseError),
    #[error(transparent)]
    ParsedUrl(#[from] ParsedUrlError),
    #[error("error converting to parsed git url {0}")]
    PixiGitUrl(String),
    #[error("while checking freshness {0}")]
    FreshnessError(std::io::Error),
}

/// Check if a package needs to be reinstalled
pub(crate) fn need_reinstall(
    installed: &InstalledDist,
    locked: &PypiPackageData,
    lock_file_dir: &Path,
) -> Result<ValidateCurrentInstall, NeedsReinstallError> {
    // Check if the installed version is the same as the required version
    match installed {
        InstalledDist::Registry(reg) => {
            if !matches!(locked.location, UrlOrPath::Url(_)) {
                return Ok(ValidateCurrentInstall::Reinstall(
                    NeedReinstall::SourceMismatch {
                        locked_location: locked.location.to_string(),
                        installed_location: "registry".to_string(),
                    },
                ));
            }

            let specifier = to_uv_version(&locked.version)?;

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
                uv_pypi_types::DirectUrl::LocalDirectory {
                    url,
                    dir_info,
                    subdirectory: _,
                } => {
                    // Recreate file url
                    let result = Url::parse(&url);
                    match result {
                        Ok(url) => {
                            // Convert the locked location, which can be a path or a url, to a url
                            let locked_url = match &locked.location {
                                // Fine if it is already a url
                                UrlOrPath::Url(url) => url.clone(),
                                // Do some path mangling if it is actually a path to get it into a url
                                UrlOrPath::Path(path) => {
                                    let path = PathBuf::from(path.as_str());
                                    // Because the path we are comparing to is absolute we need to convert
                                    let path = if path.is_absolute() {
                                        path
                                    } else {
                                        // Relative paths will be relative to the lock file directory
                                        lock_file_dir.join(path)
                                    };
                                    // Okay, now convert to a file path, if we cant do that we need to re-install
                                    match Url::from_file_path(path.clone()) {
                                        Ok(url) => url,
                                        Err(_) => {
                                            return Ok(ValidateCurrentInstall::Reinstall(
                                                NeedReinstall::UnableToConvertLockedPath {
                                                    path: path.display().to_string(),
                                                },
                                            ));
                                        }
                                    }
                                }
                            };

                            // Check if the urls are different
                            if url == locked_url {
                                // Okay so these are the same, but we need to check if the cache is newer
                                // than the source directory
                                if !check_url_freshness(&url, installed)
                                    .map_err(NeedsReinstallError::FreshnessError)?
                                {
                                    return Ok(ValidateCurrentInstall::Reinstall(
                                        NeedReinstall::SourceDirectoryNewerThanCache,
                                    ));
                                }
                            } else {
                                return Ok(ValidateCurrentInstall::Reinstall(
                                    NeedReinstall::UrlMismatch {
                                        installed_url: url.to_string(),
                                        locked_url: locked.location.as_url().map(|u| u.to_string()),
                                    },
                                ));
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
                                locked_editable: locked.editable,
                                installed_editable: dir_info.editable.unwrap_or_default(),
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
                    let lock_file_dir = typed_path::Utf8TypedPathBuf::from(
                        lock_file_dir.to_string_lossy().as_ref(),
                    );
                    let locked_url = match &locked.location {
                        // Remove `direct+` scheme if it is there so we can compare the required to
                        // the installed url
                        UrlOrPath::Url(url) => strip_direct_scheme(url).into_owned(),
                        UrlOrPath::Path(path) => {
                            let path = if path.is_absolute() {
                                path.clone()
                            } else {
                                // Relative paths will be relative to the lock file directory
                                lock_file_dir.join(path).normalize()
                            };
                            let url = Url::from_file_path(PathBuf::from(path.as_str()));
                            match url {
                                Ok(url) => url,
                                Err(_) => {
                                    return Ok(ValidateCurrentInstall::Reinstall(
                                        NeedReinstall::UnableToParseFileUrl {
                                            url: path.to_string(),
                                        },
                                    ));
                                }
                            }
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

                    if locked_url == installed_url {
                        // Check cache freshness
                        if !check_url_freshness(&locked_url, installed)
                            .map_err(NeedsReinstallError::FreshnessError)?
                        {
                            return Ok(ValidateCurrentInstall::Reinstall(
                                NeedReinstall::ArchiveDistNewerThanCache,
                            ));
                        }
                    } else {
                        return Ok(ValidateCurrentInstall::Reinstall(
                            NeedReinstall::UrlMismatch {
                                installed_url: installed_url.to_string(),
                                locked_url: locked.location.as_url().map(|u| u.to_string()),
                            },
                        ));
                    }
                }
                uv_pypi_types::DirectUrl::VcsUrl {
                    url,
                    vcs_info,
                    subdirectory: _,
                } => {
                    // Check if the installed git url is the same as the locked git url
                    // if this fails, it should be an error, because then installed url is not a git url
                    let installed_git_url = ParsedGitUrl::try_from(
                        uv_redacted::DisplaySafeUrl::from(Url::parse(url.as_str())?),
                    )?;

                    // Try to parse the locked git url, this can be any url, so this may fail
                    // in practice it always seems to succeed, even with a non-git url
                    let locked_git_url = match &locked.location {
                        UrlOrPath::Url(url) => {
                            // is it a git url?
                            if LockedGitUrl::is_locked_git_url(url) {
                                let locked_git_url = LockedGitUrl::new(url.clone());
                                to_parsed_git_url(&locked_git_url)
                                    // Needs the conversion because of a miette error
                                    .map_err(|e| NeedsReinstallError::PixiGitUrl(e.to_string()))
                            } else {
                                // it is not a git url, so we fallback to use the url as is
                                ParsedGitUrl::try_from(uv_redacted::DisplaySafeUrl::from(
                                    url.clone(),
                                ))
                                .map_err(|e: uv_pypi_types::ParsedUrlError| e.into())
                            }
                        }
                        UrlOrPath::Path(_path) => {
                            // Previously
                            return Ok(ValidateCurrentInstall::Reinstall(
                                NeedReinstall::GitArchiveIsPath,
                            ));
                        }
                    };
                    match locked_git_url {
                        Ok(locked_git_url) => {
                            // Check the repository base url with the locked url
                            let installed_repository_url =
                                RepositoryUrl::new(installed_git_url.url.repository());
                            let locked_repository_url =
                                RepositoryUrl::new(locked_git_url.url.repository());
                            if locked_repository_url != installed_repository_url {
                                // This happens when this is not a git url
                                return Ok(ValidateCurrentInstall::Reinstall(
                                    NeedReinstall::UrlMismatch {
                                        installed_url: installed_git_url.url.to_string(),
                                        locked_url: Some(locked_git_url.url.to_string()),
                                    },
                                ));
                            }
                            if vcs_info.requested_revision
                                != locked_git_url
                                    .url
                                    .reference()
                                    .as_str()
                                    .map(|s| s.to_string())
                            {
                                // The commit id is different, we need to reinstall
                                return Ok(ValidateCurrentInstall::Reinstall(
                                    NeedReinstall::GitRevMismatch {
                                        installed_rev: vcs_info
                                            .requested_revision
                                            .unwrap_or_default(),
                                        locked_rev: locked_git_url.url.reference().to_string(),
                                    },
                                ));
                            }

                            if let (Some(installed_commit), Some(locked_commit)) =
                                (vcs_info.commit_id, locked_git_url.url.precise())
                            {
                                if installed_commit != locked_commit.as_str() {
                                    return Ok(ValidateCurrentInstall::Reinstall(
                                        NeedReinstall::GitRevMismatch {
                                            installed_rev: installed_commit,
                                            locked_rev: locked_commit.to_string(),
                                        },
                                    ));
                                }
                            }
                        }
                        Err(_) => {
                            return Ok(ValidateCurrentInstall::Reinstall(
                                NeedReinstall::UnableToParseGitUrl {
                                    url: locked
                                        .location
                                        .as_url()
                                        .map(|u| u.to_string())
                                        .unwrap_or_default(),
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
    let metadata = match installed.metadata() {
        Ok(metadata) => metadata,
        Err(err) => {
            // Can't be sure lets reinstall
            return Ok(ValidateCurrentInstall::Reinstall(
                NeedReinstall::UnableToGetInstalledDistMetadata {
                    cause: err.to_string(),
                },
            ));
        }
    };

    if let Some(requires_python) = metadata.requires_python {
        // If the installed package requires a different requires python version of the locked package,
        // or if one of them is `Some` and the other is `None`.
        match &locked.requires_python {
            Some(locked_requires_python) => {
                if requires_python.to_string() != locked_requires_python.to_string() {
                    return Ok(ValidateCurrentInstall::Reinstall(
                        NeedReinstall::RequiredPythonChanged {
                            installed_python_require: requires_python.to_string(),
                            locked_python_version: locked_requires_python.to_string(),
                        },
                    ));
                }
            }
            None => {
                return Ok(ValidateCurrentInstall::Reinstall(
                    NeedReinstall::RequiredPythonChanged {
                        installed_python_require: requires_python.to_string(),
                        locked_python_version: "None".to_string(),
                    },
                ));
            }
        }
    } else if let Some(requires_python) = &locked.requires_python {
        return Ok(ValidateCurrentInstall::Reinstall(
            NeedReinstall::RequiredPythonChanged {
                installed_python_require: "None".to_string(),
                locked_python_version: requires_python.to_string(),
            },
        ));
    }

    Ok(ValidateCurrentInstall::Keep)
}
