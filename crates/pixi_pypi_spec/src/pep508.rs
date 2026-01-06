use crate::utils::extract_directory_from_url;
use crate::{Pep508ToPyPiRequirementError, PixiPypiSource, PixiPypiSpec, VersionOrStar};
use pixi_git::GitUrl;
use pixi_spec::GitSpec;
use std::path::Path;

/// Implement from [`pep508_rs::Requirement`] to make the conversion easier.
impl TryFrom<pep508_rs::Requirement> for PixiPypiSpec {
    type Error = Pep508ToPyPiRequirementError;
    fn try_from(req: pep508_rs::Requirement) -> Result<Self, Self::Error> {
        let converted = if let Some(version_or_url) = req.version_or_url {
            match version_or_url {
                pep508_rs::VersionOrUrl::VersionSpecifier(v) => {
                    PixiPypiSpec::with_extras_and_markers(
                        PixiPypiSource::Registry {
                            version: v.into(),
                            index: None,
                        },
                        req.extras,
                        req.marker,
                    )
                }
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

                                PixiPypiSpec::with_extras_and_markers(
                                    PixiPypiSource::Git { git: git_spec },
                                    req.extras,
                                    req.marker,
                                )
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
                        PixiPypiSpec::with_extras_and_markers(
                            PixiPypiSource::Git { git: git_spec },
                            req.extras,
                            req.marker,
                        )
                    } else if url.scheme().eq_ignore_ascii_case("file") {
                        // Convert the file url to a path.
                        let file = url.to_file_path().map_err(|_| {
                            Pep508ToPyPiRequirementError::PathUrlIntoPath(url.clone())
                        })?;
                        PixiPypiSpec::with_extras_and_markers(
                            PixiPypiSource::Path {
                                path: file,
                                editable: None,
                            },
                            req.extras,
                            req.marker,
                        )
                    } else {
                        let subdirectory = extract_directory_from_url(&url);
                        PixiPypiSpec::with_extras_and_markers(
                            PixiPypiSource::Url { url, subdirectory },
                            req.extras,
                            req.marker,
                        )
                    }
                }
            }
        } else if !req.extras.is_empty() {
            PixiPypiSpec::with_extras_and_markers(
                PixiPypiSource::Registry {
                    version: VersionOrStar::Star,
                    index: None,
                },
                req.extras,
                req.marker,
            )
        } else {
            PixiPypiSpec::new(PixiPypiSource::Registry {
                version: VersionOrStar::Star,
                index: None,
            })
        };
        Ok(converted)
    }
}
