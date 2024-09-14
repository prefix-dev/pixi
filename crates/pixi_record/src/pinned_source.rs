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

/// A pinned version of a [`pixi_spec::UrlSourceSpec`].
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

/// A pinned version of a [`pixi_spec::GitSpec`].
#[derive(Debug, Clone)]
pub struct PinnedGitSpec {
    pub git: Url,
    pub commit: String,
    pub rev: Option<GitReference>,
}

/// A reference to a specific commit in a git repository.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum GitReference {
    /// The HEAD commit of a branch.
    Branch(String),

    /// A specific tag.
    Tag(String),

    /// A specific commit.
    Rev(String),
}

impl From<PinnedGitSpec> for PinnedSourceSpec {
    fn from(value: PinnedGitSpec) -> Self {
        PinnedSourceSpec::Git(value)
    }
}

/// A pinned version of a [`pixi_spec::PathSourceSpec`].
#[derive(Debug, Clone)]
pub struct PinnedPathSpec {
    pub path: Utf8TypedPathBuf,
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
