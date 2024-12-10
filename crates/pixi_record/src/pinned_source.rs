use std::{
    fmt::{Display, Formatter},
    path::{Path, PathBuf},
};

use pixi_spec::{GitSpec, PathSourceSpec, Reference, SourceSpec, UrlSourceSpec};
use rattler_digest::{Md5Hash, Sha256Hash};
use rattler_lock::UrlOrPath;
use thiserror::Error;
use typed_path::Utf8TypedPathBuf;
use url::Url;

/// Describes an exact revision of a source checkout. This is used to pin a
/// particular source definition to a revision. A git source spec does not
/// describe an exact commit. This struct describes an exact commit.
#[derive(Debug, Clone)]
pub enum PinnedSourceSpec {
    Url(PinnedUrlSpec),
    Git(PinnedGitSpec),
    Path(PinnedPathSpec),
}

/// Describes a mutable source spec. This is similar to a [`PinnedSourceSpec`]
/// but the contents can change over time.
#[derive(Debug, Clone)]
pub enum MutablePinnedSourceSpec {
    Path(PinnedPathSpec),
}

impl PinnedSourceSpec {
    pub fn as_path(&self) -> Option<&PinnedPathSpec> {
        match self {
            PinnedSourceSpec::Path(spec) => Some(spec),
            _ => None,
        }
    }

    pub fn as_url(&self) -> Option<&PinnedUrlSpec> {
        match self {
            PinnedSourceSpec::Url(spec) => Some(spec),
            _ => None,
        }
    }

    pub fn as_git(&self) -> Option<&PinnedGitSpec> {
        match self {
            PinnedSourceSpec::Git(spec) => Some(spec),
            _ => None,
        }
    }

    pub fn into_path(self) -> Option<PinnedPathSpec> {
        match self {
            PinnedSourceSpec::Path(spec) => Some(spec),
            _ => None,
        }
    }

    pub fn into_url(self) -> Option<PinnedUrlSpec> {
        match self {
            PinnedSourceSpec::Url(spec) => Some(spec),
            _ => None,
        }
    }

    pub fn into_git(self) -> Option<PinnedGitSpec> {
        match self {
            PinnedSourceSpec::Git(spec) => Some(spec),
            _ => None,
        }
    }

    /// Converts this instance into a [`MutablePinnedSourceSpec`], or if this
    /// instance does not refer to mutable source the original
    /// [`PinnedSourceSpec`].
    ///
    /// A mutable source is a source that can change over time. For example, a
    /// local path.
    #[allow(clippy::result_large_err)]
    pub fn into_mutable(self) -> Result<MutablePinnedSourceSpec, PinnedSourceSpec> {
        match self {
            PinnedSourceSpec::Path(spec) => Ok(MutablePinnedSourceSpec::Path(spec)),
            _ => Err(self),
        }
    }

    /// Returns true if the pinned source will never change. This can be useful
    /// for caching purposes.
    pub fn is_immutable(&self) -> bool {
        !matches!(self, PinnedSourceSpec::Path(_))
    }
}

impl MutablePinnedSourceSpec {
    /// Returns the path spec if this instance is a path spec.
    pub fn as_path(&self) -> Option<&PinnedPathSpec> {
        match self {
            MutablePinnedSourceSpec::Path(spec) => Some(spec),
        }
    }

    /// Returns the path spec if this instance is a path spec.
    pub fn into_path(self) -> Option<PinnedPathSpec> {
        match self {
            MutablePinnedSourceSpec::Path(spec) => Some(spec),
        }
    }
}

impl From<MutablePinnedSourceSpec> for PinnedSourceSpec {
    fn from(value: MutablePinnedSourceSpec) -> Self {
        match value {
            MutablePinnedSourceSpec::Path(spec) => PinnedSourceSpec::Path(spec),
        }
    }
}

/// A pinned url archive.
#[derive(Debug, Clone)]
pub struct PinnedUrlSpec {
    pub url: Url,
    pub sha256: Sha256Hash,
    pub md5: Option<Md5Hash>,
}

impl From<PinnedUrlSpec> for PinnedSourceSpec {
    fn from(value: PinnedUrlSpec) -> Self {
        PinnedSourceSpec::Url(value)
    }
}

/// A pinned version of a git checkout.
#[derive(Debug, Clone)]
pub struct PinnedGitSpec {
    pub git: Url,
    pub commit: String,
    pub rev: Option<Reference>,
    pub subdirectory: Option<String>,
}

impl From<PinnedGitSpec> for PinnedSourceSpec {
    fn from(value: PinnedGitSpec) -> Self {
        PinnedSourceSpec::Git(value)
    }
}

/// A pinned version of a path based source dependency.
#[derive(Debug, Clone)]
pub struct PinnedPathSpec {
    pub path: Utf8TypedPathBuf,
}

impl PinnedPathSpec {
    /// Resolves the path to an absolute path.
    pub fn resolve(&self, project_root: &Path) -> PathBuf {
        let native_path = Path::new(self.path.as_str());
        if native_path.is_absolute() {
            native_path.to_path_buf()
        } else {
            project_root.join(native_path)
        }
    }
}

impl From<PinnedPathSpec> for PinnedSourceSpec {
    fn from(value: PinnedPathSpec) -> Self {
        PinnedSourceSpec::Path(value)
    }
}

impl From<PinnedSourceSpec> for UrlOrPath {
    fn from(value: PinnedSourceSpec) -> Self {
        match value {
            PinnedSourceSpec::Url(spec) => spec.into(),
            PinnedSourceSpec::Git(spec) => spec.into(),
            PinnedSourceSpec::Path(spec) => spec.into(),
        }
    }
}

impl From<PinnedPathSpec> for UrlOrPath {
    fn from(value: PinnedPathSpec) -> Self {
        UrlOrPath::Path(value.path)
    }
}

impl From<PinnedGitSpec> for UrlOrPath {
    fn from(_value: PinnedGitSpec) -> Self {
        // TODO: implement this first
        unimplemented!()
    }
}

impl From<PinnedUrlSpec> for UrlOrPath {
    fn from(_value: PinnedUrlSpec) -> Self {
        unimplemented!()
    }
}

#[derive(Debug, Error)]
pub enum ParseError {}

impl TryFrom<UrlOrPath> for PinnedSourceSpec {
    type Error = ParseError;

    fn try_from(value: UrlOrPath) -> Result<Self, Self::Error> {
        match value {
            UrlOrPath::Url(_) => unimplemented!(),
            UrlOrPath::Path(path) => Ok(PinnedPathSpec { path }.into()),
        }
    }
}

#[derive(Debug, Error)]
pub enum SourceMismatchError {
    #[error("the locked path '{locked}' does not match the requested path '{requested}'")]
    PathMismatch {
        locked: Utf8TypedPathBuf,
        requested: Utf8TypedPathBuf,
    },

    #[error("the locked url '{locked}' does not match the requested url '{requested}'")]
    UrlMismatch { locked: Url, requested: Url },

    #[error("the locked {hash} of url '{url}' ({locked}) does not match the requested {hash} ({requested})")]
    UrlHashMismatch {
        hash: &'static str,
        url: Url,
        locked: String,
        requested: String,
    },

    #[error("the locked git rev '{locked}' for '{git}' does not match the requested git rev '{requested}'")]
    GitRevMismatch {
        git: Url,
        locked: String,
        requested: String,
    },

    #[error("the locked source type does not match the requested type")]
    SourceTypeMismatch,
}

impl PinnedPathSpec {
    #[allow(clippy::result_large_err)]
    pub fn satisfies(&self, spec: &PathSourceSpec) -> Result<(), SourceMismatchError> {
        if spec.path != self.path {
            return Err(SourceMismatchError::PathMismatch {
                locked: self.path.clone(),
                requested: spec.path.clone(),
            });
        }
        Ok(())
    }
}

impl PinnedUrlSpec {
    #[allow(clippy::result_large_err)]
    pub fn satisfies(&self, spec: &UrlSourceSpec) -> Result<(), SourceMismatchError> {
        if spec.url != self.url {
            return Err(SourceMismatchError::UrlMismatch {
                locked: self.url.clone(),
                requested: spec.url.clone(),
            });
        }
        if let Some(sha256) = &spec.sha256 {
            if *sha256 != self.sha256 {
                return Err(SourceMismatchError::UrlHashMismatch {
                    hash: "sha256",
                    url: self.url.clone(),
                    locked: format!("{:x}", self.sha256),
                    requested: format!("{:x}", sha256),
                });
            }
        }
        if let Some(md5) = &spec.md5 {
            if Some(md5) != self.md5.as_ref() {
                return Err(SourceMismatchError::UrlHashMismatch {
                    hash: "md5",
                    url: self.url.clone(),
                    locked: self
                        .md5
                        .map_or("None".to_string(), |md5| format!("{:x}", md5)),
                    requested: format!("{:x}", md5),
                });
            }
        }
        Ok(())
    }
}

impl PinnedGitSpec {
    #[allow(clippy::result_large_err)]
    pub fn satisfies(&self, spec: &GitSpec) -> Result<(), SourceMismatchError> {
        // TODO: Normalize the git urls before comparing.
        if spec.git != self.git {
            return Err(SourceMismatchError::UrlMismatch {
                locked: self.git.clone(),
                requested: spec.git.clone(),
            });
        }

        let locked_git_ref = self
            .rev
            .clone()
            .unwrap_or_else(|| Reference::Rev(self.commit.clone()));

        if let Some(requested_ref) = &spec.rev {
            if requested_ref != &locked_git_ref {
                return Err(SourceMismatchError::GitRevMismatch {
                    git: self.git.clone(),
                    locked: locked_git_ref.to_string(),
                    requested: requested_ref.to_string(),
                });
            }
        }
        Ok(())
    }
}

impl PinnedSourceSpec {
    #[allow(clippy::result_large_err)]
    pub fn satisfies(&self, spec: &SourceSpec) -> Result<(), SourceMismatchError> {
        match (self, spec) {
            (PinnedSourceSpec::Path(locked), SourceSpec::Path(spec)) => locked.satisfies(spec),
            (PinnedSourceSpec::Url(locked), SourceSpec::Url(spec)) => locked.satisfies(spec),
            (PinnedSourceSpec::Git(locked), SourceSpec::Git(spec)) => locked.satisfies(spec),
            (_, _) => Err(SourceMismatchError::SourceTypeMismatch),
        }
    }
}

impl Display for PinnedSourceSpec {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            PinnedSourceSpec::Path(spec) => write!(f, "{}", spec.path),
            PinnedSourceSpec::Url(spec) => write!(f, "{}", spec.url),
            PinnedSourceSpec::Git(spec) => write!(f, "{}", spec.git),
        }
    }
}

impl Display for PinnedUrlSpec {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.url)
    }
}

impl Display for PinnedPathSpec {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.path)
    }
}

impl Display for PinnedGitSpec {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.git, self.commit)
    }
}
