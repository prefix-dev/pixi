use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

// use pep440_rs::VersionSpecifiers;
use pixi_manifest::{
    pypi::{pypi_requirement::ParsedGitUrl, GitRev, VersionOrStar},
    PyPiRequirement,
};
use thiserror::Error;
use url::Url;
use uv_distribution_filename::DistExtension;
use uv_git::{GitReference, GitSha};
use uv_normalize::{InvalidNameError, PackageName};
use uv_pep440::VersionSpecifiers;
use uv_pep508::VerbatimUrl;
use uv_pypi_types::RequirementSource;

use super::to_git_reference;

/// Create a url that uv can use to install a version
fn create_uv_url(
    url: &Url,
    rev: Option<&GitRev>,
    subdir: Option<&str>,
) -> Result<Url, url::ParseError> {
    // Create the url.
    let url = format!("git+{url}");
    // Add the tag or rev if it exists.
    let url = rev
        .as_ref()
        .map_or_else(|| url.clone(), |tag_or_rev| format!("{url}@{}", tag_or_rev));

    // Add the subdirectory if it exists.
    let url = subdir.as_ref().map_or_else(
        || url.clone(),
        |subdir| format!("{url}#subdirectory={subdir}"),
    );
    url.parse()
}

fn to_version_specificers(
    version: &VersionOrStar,
) -> Result<VersionSpecifiers, uv_pep440::VersionSpecifiersParseError> {
    match version {
        VersionOrStar::Version(v) => VersionSpecifiers::from_str(&v.to_string()),
        VersionOrStar::Star => Ok(VersionSpecifiers::from_iter(vec![])),
    }
}

#[derive(Error, Debug)]
pub enum AsPep508Error {
    #[error("error while canonicalizing {path}")]
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
    #[error("error while canonicalizing {0}")]
    VerabatimUrlError(#[from] uv_pep508::VerbatimUrlError),
    #[error("error in extension parsing")]
    ExtensionError(#[from] uv_distribution_filename::ExtensionError),
    #[error("error in parsing version specificers")]
    VersionSpecifiersError(#[from] uv_pep440::VersionSpecifiersParseError),
}

/// Convert into a `pypi_types::Requirement`, which is an uv extended
/// requirement type
pub fn as_uv_req(
    req: &PyPiRequirement,
    name: &str,
    project_root: &Path,
) -> Result<uv_pypi_types::Requirement, AsPep508Error> {
    let name = PackageName::new(name.to_owned())?;
    let source = match req {
        PyPiRequirement::Version { version, index, .. } => {
            // TODO: implement index later
            RequirementSource::Registry {
                specifier: to_version_specificers(version)?,
                index: index.clone(),
            }
        }
        PyPiRequirement::Git {
            url:
                ParsedGitUrl {
                    git,
                    rev,
                    tag,
                    subdirectory,
                    branch,
                },
            ..
        } => RequirementSource::Git {
            repository: git.clone(),
            precise: rev
                .as_ref()
                .map(|s| s.as_full())
                .and_then(|s| s.map(GitSha::from_str))
                .transpose()
                .expect("could not parse sha"),
            reference: tag
                .as_ref()
                .map(|tag| GitReference::Tag(tag.clone()))
                .or(branch
                    .as_ref()
                    .map(|branch| GitReference::Branch(branch.to_string())))
                .or(rev.as_ref().map(to_git_reference))
                .unwrap_or(GitReference::DefaultBranch),
            subdirectory: subdirectory.as_ref().and_then(|s| s.parse().ok()),
            url: VerbatimUrl::from_url(
                create_uv_url(git, rev.as_ref(), subdirectory.as_deref()).map_err(|e| {
                    AsPep508Error::UrlParseError {
                        source: e,
                        url: git.to_string(),
                    }
                })?,
            ),
        },
        PyPiRequirement::Path {
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
                    install_path: canonicalized,
                    editable: editable.unwrap_or_default(),
                    url: verbatim,
                    // TODO: we could see if we ever need this
                    // AFAICS it would be useful for constrainging dependencies
                    r#virtual: false,
                }
            } else if *editable == Some(true) {
                {
                    return Err(AsPep508Error::EditableIsNotDir {
                        path: canonicalized,
                    });
                }
            } else {
                RequirementSource::Path {
                    install_path: canonicalized,
                    url: verbatim,
                    ext: DistExtension::from_path(path)?,
                }
            }
        }
        PyPiRequirement::Url {
            url, subdirectory, ..
        } => RequirementSource::Url {
            subdirectory: subdirectory.as_ref().map(|sub| PathBuf::from(sub.as_str())),
            location: url.clone(),
            url: VerbatimUrl::from_url(url.clone()),
            ext: DistExtension::from_path(url.path())?,
        },
        PyPiRequirement::RawVersion(version) => RequirementSource::Registry {
            specifier: to_version_specificers(version)?,
            index: None,
        },
    };

    Ok(uv_pypi_types::Requirement {
        name: name.clone(),
        extras: req
            .extras()
            .iter()
            .map(|e| uv_pep508::ExtraName::new(e.to_string()).expect("conversion failed"))
            .collect(),
        marker: Default::default(),
        source,
        origin: None,
    })
}
