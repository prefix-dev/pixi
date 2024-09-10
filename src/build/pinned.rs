use pixi_spec::GitReference;
use rattler_digest::{Md5Hash, Sha256Hash};
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
