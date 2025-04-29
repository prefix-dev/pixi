use crate::utils::extract_directory_from_url;
use crate::{Pep508ToPyPiRequirementError, PixiPypiSpec, VersionOrStar};
use pixi_git::GitUrl;
use pixi_spec::GitSpec;
use std::path::Path;

/// Implement from [`pep508_rs::Requirement`] to make the conversion easier.
impl TryFrom<pep508_rs::Requirement> for PixiPypiSpec {
    type Error = Pep508ToPyPiRequirementError;
    fn try_from(req: pep508_rs::Requirement) -> Result<Self, Self::Error> {
        let converted = if let Some(version_or_url) = req.version_or_url {
            match version_or_url {
                pep508_rs::VersionOrUrl::VersionSpecifier(v) => PixiPypiSpec::Version {
                    version: v.into(),
                    extras: req.extras,
                    index: None,
                },
                pep508_rs::VersionOrUrl::Url(u) => {
                    let url = u.to_url();
                    if let Some((prefix, ..)) = url.scheme().split_once('+') {
                        match prefix {
                            "git" => {
                                let subdirectory = extract_directory_from_url(&url);
                                let git_url = GitUrl::try_from(url).unwrap();
                                let git_spec = GitSpec {
                                    git: git_url.repository().clone(),
                                    rev: Some(git_url.reference().clone().into()),
                                    subdirectory,
                                };

                                Self::Git {
                                    url: git_spec,
                                    extras: req.extras,
                                }
                            }
                            "bzr" => {
                                return Err(Pep508ToPyPiRequirementError::UnsupportedUrlPrefix {
                                    prefix: prefix.to_string(),
                                    url: u.to_url(),
                                    message: "Bazaar is not supported",
                                });
                            }
                            "hg" => {
                                return Err(Pep508ToPyPiRequirementError::UnsupportedUrlPrefix {
                                    prefix: prefix.to_string(),
                                    url: u.to_url(),
                                    message: "Bazaar is not supported",
                                });
                            }
                            "svn" => {
                                return Err(Pep508ToPyPiRequirementError::UnsupportedUrlPrefix {
                                    prefix: prefix.to_string(),
                                    url: u.to_url(),
                                    message: "Bazaar is not supported",
                                });
                            }
                            _ => {
                                return Err(Pep508ToPyPiRequirementError::UnsupportedUrlPrefix {
                                    prefix: prefix.to_string(),
                                    url: u.to_url(),
                                    message: "Unknown scheme",
                                });
                            }
                        }
                    } else if Path::new(url.path())
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("git"))
                    {
                        let git_url = GitUrl::try_from(url).unwrap();
                        let subdirectory = extract_directory_from_url(git_url.repository());
                        let git_spec = GitSpec {
                            git: git_url.repository().clone(),
                            rev: Some(git_url.reference().clone().into()),
                            subdirectory,
                        };
                        Self::Git {
                            url: git_spec,
                            extras: req.extras,
                        }
                    } else if url.scheme().eq_ignore_ascii_case("file") {
                        // Convert the file url to a path.
                        let file = url.to_file_path().map_err(|_| {
                            Pep508ToPyPiRequirementError::PathUrlIntoPath(url.clone())
                        })?;
                        PixiPypiSpec::Path {
                            path: file,
                            editable: None,
                            extras: req.extras,
                        }
                    } else {
                        let subdirectory = extract_directory_from_url(&url);
                        PixiPypiSpec::Url {
                            url,
                            extras: req.extras,
                            subdirectory,
                        }
                    }
                }
            }
        } else if !req.extras.is_empty() {
            PixiPypiSpec::Version {
                version: VersionOrStar::Star,
                extras: req.extras,
                index: None,
            }
        } else {
            PixiPypiSpec::RawVersion(VersionOrStar::Star)
        };
        Ok(converted)
    }
}
