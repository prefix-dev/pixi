use pixi_pypi_spec::{PixiPypiSpec, VersionOrStar};
use pixi_spec::{GitReference, GitSpec};
use rattler_lock::UrlOrPath;
use std::{
    path::{Path, PathBuf},
    str::FromStr,
};
use thiserror::Error;
use url::Url;
use uv_distribution_filename::DistExtension;
use uv_distribution_types::RequirementSource;
use uv_normalize::{InvalidNameError, PackageName};
use uv_pep440::VersionSpecifiers;
use uv_pep508::VerbatimUrl;
use uv_pypi_types::{ParsedPathUrl, ParsedUrl, VerbatimParsedUrl};
use uv_redacted::DisplaySafeUrl;

use crate::{
    ConversionError, GitUrlWithPrefix, into_uv_git_reference, to_uv_marker_tree,
    to_uv_version_specifiers,
};

/// Create a url that uv can use to install a version
fn create_uv_url(
    url: &Url,
    rev: Option<&GitReference>,
    subdir: Option<&str>,
) -> Result<Url, url::ParseError> {
    // Add the git+ prefix if it doesn't exist.
    let url = url.to_string();
    let url = match url.strip_prefix("git+") {
        Some(_) => url,
        None => format!("git+{}", url),
    };

    // Add the tag or rev if it exists.
    let url = rev.as_ref().map_or_else(
        || url.clone(),
        |tag_or_rev| {
            if !tag_or_rev.is_default() {
                format!("{url}@{}", tag_or_rev)
            } else {
                url.clone()
            }
        },
    );

    // Add the subdirectory if it exists.
    let url = subdir.as_ref().map_or_else(
        || url.clone(),
        |subdir| format!("{url}#subdirectory={subdir}"),
    );
    url.parse()
}

fn manifest_version_to_version_specifiers(
    version: &VersionOrStar,
) -> Result<VersionSpecifiers, uv_pep440::VersionSpecifiersParseError> {
    match version {
        VersionOrStar::Version(v) => VersionSpecifiers::from_str(&v.to_string()),
        VersionOrStar::Star => Ok(VersionSpecifiers::from_iter(vec![])),
    }
}

#[derive(Error, Debug)]
pub enum AsPep508Error {
    #[error("error while canonicalization {path}")]
    CanonicalizeError {
        source: std::io::Error,
        path: PathBuf,
    },
    #[error("parsing url {url}")]
    UrlParseError {
        source: url::ParseError,
        url: String,
    },
    #[error("invalid name: {0}")]
    NameError(#[from] InvalidNameError),
    #[error("using an editable flag for a path that is not a directory: {path}")]
    EditableIsNotDir { path: PathBuf },
    #[error("error while canonicalization {0}")]
    VerbatimUrlError(#[from] uv_pep508::VerbatimUrlError),
    #[error("error in extension parsing")]
    ExtensionError(#[from] uv_distribution_filename::ExtensionError),
    #[error("error in parsing version specifiers")]
    VersionSpecifiersError(#[from] uv_pep440::VersionSpecifiersParseError),
    #[error(transparent)]
    GitUrlParseError(#[from] uv_git_types::GitUrlParseError),
}

/// Convert into a [`uv_distribution_types::Requirement`], which is an uv extended
/// requirement type
pub fn as_uv_req(
    req: &PixiPypiSpec,
    name: &str,
    project_root: &Path,
) -> Result<uv_distribution_types::Requirement, AsPep508Error> {
    let name = PackageName::from_str(name)?;
    let source = match req {
        PixiPypiSpec::Version { version, index, .. } => {
            // TODO: implement index later
            RequirementSource::Registry {
                specifier: manifest_version_to_version_specifiers(version)?,
                index: index.clone().map(|url| {
                    uv_distribution_types::IndexMetadata::from(
                        uv_distribution_types::IndexUrl::from(VerbatimUrl::from_url(url.into())),
                    )
                }),
                conflict: None,
            }
        }
        PixiPypiSpec::Git {
            url:
                GitSpec {
                    git,
                    rev,
                    subdirectory,
                },
            ..
        } => {
            let git_url = GitUrlWithPrefix::from(git);

            RequirementSource::Git {
                // Url is already a git url, should look like:
                // - 'ssh://git@github.com/user/repo'
                // - 'https://github.com/user/repo'
                git: uv_git_types::GitUrl::from_fields(
                    git_url.to_display_safe_url(),
                    // The reference to the commit to use, which could be a branch, tag or revision.
                    rev.as_ref()
                        .map(|rev| into_uv_git_reference(rev.clone().into()))
                        .unwrap_or(uv_git_types::GitReference::DefaultBranch),
                    // Unique identifier for the commit, as Git object identifier
                    rev.as_ref()
                        .map(|s| s.as_full_commit())
                        .and_then(|s| s.map(uv_git_types::GitOid::from_str))
                        .transpose()
                        .expect("could not parse sha"),
                )?,
                subdirectory: subdirectory
                    .as_ref()
                    .map(|s| PathBuf::from(s).into_boxed_path()),
                // The full url used to clone, comparable to the git+ url in pip. e.g:
                // - 'git+SCHEMA://HOST/PATH@REF#subdirectory=SUBDIRECTORY'
                // - 'git+ssh://github.com/user/repo@d099af3b1028b00c232d8eda28a997984ae5848b'
                url: {
                    let created_url = create_uv_url(
                        git_url.without_git_prefix(),
                        rev.as_ref(),
                        subdirectory.as_deref(),
                    )
                    .map_err(|e| AsPep508Error::UrlParseError {
                        source: e,
                        url: git.to_string(),
                    })?;

                    VerbatimUrl::from_url(created_url.into())
                },
            }
        }
        PixiPypiSpec::Path {
            path,
            editable,
            extras: _,
        } => {
            let joined = project_root.join(path);
            let canonicalized =
                dunce::canonicalize(&joined).map_err(|e| AsPep508Error::CanonicalizeError {
                    source: e,
                    path: joined.clone(),
                })?;
            let given = path
                .to_str()
                .map(|s| s.to_owned())
                .unwrap_or_else(String::new);
            let verbatim = VerbatimUrl::from_path(path, project_root)?.with_given(given);

            if canonicalized.is_dir() {
                RequirementSource::Directory {
                    install_path: canonicalized.into_boxed_path(),
                    editable: Some(editable.unwrap_or_default()),
                    url: verbatim,
                    // TODO: we could see if we ever need this
                    // AFAICS it would be useful for constrainging dependencies
                    r#virtual: Some(false),
                }
            } else if *editable == Some(true) {
                {
                    return Err(AsPep508Error::EditableIsNotDir {
                        path: canonicalized,
                    });
                }
            } else {
                RequirementSource::Path {
                    install_path: canonicalized.into_boxed_path(),
                    url: verbatim,
                    ext: DistExtension::from_path(path)?,
                }
            }
        }
        PixiPypiSpec::Url {
            url, subdirectory, ..
        } => {
            // We will clone the original URL and strip it's SHA256 fragment,
            // So that we can normalize the URL for comparison.
            let mut location_url = url.clone();
            location_url.set_fragment(None);
            let verbatim_url = VerbatimUrl::from_url(url.clone().into());

            RequirementSource::Url {
                subdirectory: subdirectory
                    .as_ref()
                    .map(|sub| PathBuf::from(sub.as_str()).into_boxed_path()),
                location: location_url.into(),
                url: verbatim_url,
                ext: DistExtension::from_path(url.path())?,
            }
        }
        PixiPypiSpec::RawVersion(version) => RequirementSource::Registry {
            specifier: manifest_version_to_version_specifiers(version)?,
            index: None,
            conflict: None,
        },
    };

    Ok(uv_distribution_types::Requirement {
        name: name.clone(),
        extras: req
            .extras()
            .iter()
            .map(|e| uv_pep508::ExtraName::from_str(e.as_ref()).expect("conversion failed"))
            .collect(),
        marker: Default::default(),
        groups: Default::default(),
        source,
        origin: None,
    })
}

/// Convert a [`pep508_rs::Requirement`] into a [`uv_distribution_types::Requirement`]
pub fn pep508_requirement_to_uv_requirement(
    requirement: pep508_rs::Requirement,
) -> Result<uv_distribution_types::Requirement, ConversionError> {
    let parsed_url = if let Some(version_or_url) = requirement.version_or_url {
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
                        let ext =
                            DistExtension::from_path(Path::new(path.as_str())).map_err(|e| {
                                ConversionError::ExpectedArchiveButFoundPath(
                                    PathBuf::from_str(path.as_str()).expect("not a path"),
                                    e,
                                )
                            })?;
                        let parsed_url = ParsedUrl::Path(ParsedPathUrl::from_source(
                            PathBuf::from(path.as_str()).into_boxed_path(),
                            ext,
                            verbatim_url.to_url().into(),
                        ));

                        VerbatimParsedUrl {
                            parsed_url,
                            verbatim: uv_pep508::VerbatimUrl::from_url(
                                verbatim_url.raw().clone().into(),
                            )
                            .with_given(verbatim_url.given().expect("should have given string")),
                        }
                        // Can only be an archive
                    }
                    UrlOrPath::Url(u) => VerbatimParsedUrl {
                        parsed_url: ParsedUrl::try_from(DisplaySafeUrl::from(u.clone()))
                            .expect("cannot convert to url"),
                        verbatim: uv_pep508::VerbatimUrl::from_url(u.into()),
                    },
                };

                Some(uv_pep508::VersionOrUrl::Url(url))
            }
        }
    } else {
        None
    };

    let marker = to_uv_marker_tree(&requirement.marker)?;
    let converted = uv_pep508::Requirement {
        name: uv_pep508::PackageName::from_str(requirement.name.as_ref())
            .expect("cannot normalize name"),
        extras: requirement
            .extras
            .iter()
            .map(|e| uv_pep508::ExtraName::from_str(e.as_ref()).expect("cannot convert extra name"))
            .collect(),
        marker,
        version_or_url: parsed_url,
        // Don't think this needs to be set
        origin: None,
    };

    Ok(converted.into())
}

#[cfg(test)]
mod tests {
    use uv_redacted::DisplaySafeUrl;

    use super::*;

    #[test]
    fn test_git_url() {
        let pypi_req = PixiPypiSpec::Git {
            url: GitSpec {
                git: Url::parse("ssh://git@github.com/user/test.git").unwrap(),
                rev: Some(GitReference::Rev(
                    "d099af3b1028b00c232d8eda28a997984ae5848b".to_string(),
                )),
                subdirectory: None,
            },
            extras: vec![],
        };
        let uv_req = as_uv_req(&pypi_req, "test", Path::new("")).unwrap();

        let expected_uv_req = RequirementSource::Git {
            git: uv_git_types::GitUrl::from_fields(
                DisplaySafeUrl::parse("ssh://git@github.com/user/test.git").unwrap(),
                uv_git_types::GitReference::BranchOrTagOrCommit("d099af3b1028b00c232d8eda28a997984ae5848b".to_string()),
                Some(uv_git_types::GitOid::from_str("d099af3b1028b00c232d8eda28a997984ae5848b").unwrap())).unwrap(),
            subdirectory: None,
            url: VerbatimUrl::from_url(DisplaySafeUrl::parse("git+ssh://git@github.com/user/test.git@d099af3b1028b00c232d8eda28a997984ae5848b").unwrap()),
        };

        assert_eq!(
            uv_req.source, expected_uv_req,
            "Expected {} but got {}",
            expected_uv_req, uv_req.source
        );

        // With git+ prefix
        let pypi_req = PixiPypiSpec::Git {
            url: GitSpec {
                git: Url::parse("git+https://github.com/user/test.git").unwrap(),
                rev: Some(GitReference::Rev(
                    "d099af3b1028b00c232d8eda28a997984ae5848b".to_string(),
                )),
                subdirectory: None,
            },
            extras: vec![],
        };
        let uv_req = as_uv_req(&pypi_req, "test", Path::new("")).unwrap();
        let expected_uv_req = RequirementSource::Git {
            git: uv_git_types::GitUrl::from_fields(
                DisplaySafeUrl::parse("https://github.com/user/test.git").unwrap(),
                uv_git_types::GitReference::BranchOrTagOrCommit(
                    "d099af3b1028b00c232d8eda28a997984ae5848b".to_string(),
                ),
                Some(
                    uv_git_types::GitOid::from_str("d099af3b1028b00c232d8eda28a997984ae5848b")
                        .unwrap(),
                ),
            )
            .unwrap(),
            subdirectory: None,
            url: VerbatimUrl::from_url(
                DisplaySafeUrl::parse(
                    "git+https://github.com/user/test.git@d099af3b1028b00c232d8eda28a997984ae5848b",
                )
                .unwrap(),
            ),
        };
        assert_eq!(uv_req.source, expected_uv_req);
    }

    #[test]
    fn test_url_with_hash() {
        let url_with_hash =
            Url::parse("https://example.com/package.tar.gz#sha256=abc123def456").unwrap();
        let pypi_req = PixiPypiSpec::Url {
            url: url_with_hash.clone(),
            subdirectory: None,
            extras: vec![],
        };

        let uv_req = as_uv_req(&pypi_req, "test-package", Path::new("")).unwrap();

        if let RequirementSource::Url {
            location,
            url: verbatim_url,
            ..
        } = uv_req.source
        {
            // We will check that the location URL (used for comparison) has the fragment stripped
            assert!(!location.to_string().contains("sha256=abc123def456"));
            assert_eq!(location.fragment(), None);

            // But the verbatim URL should still preserve the hash (which is used for verification)
            assert!(verbatim_url.as_str().contains("sha256=abc123def456"));
            assert_eq!(url_with_hash.fragment(), Some("sha256=abc123def456"));
        } else {
            panic!("Expected RequirementSource::Url");
        }
    }
}
