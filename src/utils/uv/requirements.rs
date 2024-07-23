use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

use pep508_rs::VerbatimUrl;
use pixi_manifest::{pypi::GitRev, PyPiRequirement};
use pypi_types::RequirementSource;
use thiserror::Error;
use url::Url;
use uv_git::{GitReference, GitSha};
use uv_normalize::{InvalidNameError, PackageName};

use crate::utils::uv::to_git_reference;

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
    VerabatimUrlError(#[from] pep508_rs::VerbatimUrlError),
}

/// Convert into a `pypi_types::Requirement`, which is an uv extended
/// requirement type
pub fn as_uv_req(
    req: &PyPiRequirement,
    name: &str,
    project_root: &Path,
) -> Result<pypi_types::Requirement, AsPep508Error> {
    let name = PackageName::new(name.to_owned())?;
    let source = match req {
        PyPiRequirement::Version { version, .. } => {
            // TODO: implement index later
            RequirementSource::Registry {
                specifier: version.clone().into(),
                index: None,
            }
        }
        PyPiRequirement::Git {
            git,
            rev,
            tag,
            subdirectory,
            branch,
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
            let verbatim = VerbatimUrl::from_path(canonicalized.clone())?.with_given(given);

            // TODO: we should maybe also give an error when editable is used for something
            // that is not a directory.
            if canonicalized.is_dir() {
                RequirementSource::Directory {
                    install_path: canonicalized,
                    lock_path: path.clone(),
                    editable: editable.unwrap_or_default(),
                    url: verbatim,
                }
            } else {
                RequirementSource::Path {
                    install_path: canonicalized,
                    lock_path: path.clone(),
                    url: verbatim,
                }
            }
        }
        PyPiRequirement::Url {
            url, subdirectory, ..
        } => {
            RequirementSource::Url {
                // TODO: fill these later
                subdirectory: subdirectory.as_ref().map(|sub| PathBuf::from(sub.as_str())),
                location: url.clone(),
                url: VerbatimUrl::from_url(url.clone()),
            }
        }
        PyPiRequirement::RawVersion(version) => RequirementSource::Registry {
            specifier: version.clone().into(),
            index: None,
        },
    };

    Ok(pypi_types::Requirement {
        name: name.clone(),
        extras: req.extras().to_vec(),
        marker: None,
        source,
        origin: None,
    })
}
