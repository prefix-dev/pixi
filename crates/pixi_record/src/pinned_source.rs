use std::{
    fmt::{Display, Formatter},
    path::{Path, PathBuf},
    str::FromStr,
};

use miette::IntoDiagnostic;
use pixi_git::url::{redact_credentials, RepositoryUrl};
use pixi_git::{sha::GitSha, GitUrl};
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
#[derive(Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub struct PinnedGitCheckout {
    pub commit: GitSha,
    pub subdirectory: Option<String>,
    pub reference: Reference,
}

impl PinnedGitCheckout {
    /// Extracts a pinned git checkout from the query pairs and the hash
    /// fragment in the given URL.
    fn from_locked_url(locked_url: &LockedGitUrl) -> miette::Result<PinnedGitCheckout> {
        let url = &locked_url.0;
        let mut reference = Reference::DefaultBranch;
        let mut subdirectory = None;
        for (key, val) in url.query_pairs() {
            match &*key {
                "tag" => reference = Reference::Tag(val.into_owned()),
                "branch" => reference = Reference::Branch(val.into_owned()),
                "rev" => reference = Reference::Rev(val.into_owned()),
                // If the URL points to a subdirectory, extract it, as in (git):
                //   `git+https://git.example.com/MyProject.git@v1.0#subdirectory=pkg_dir`
                //   `git+https://git.example.com/MyProject.git@v1.0#egg=pkg&subdirectory=pkg_dir`
                "subdirectory" => subdirectory = Some(val.into_owned()),
                _ => continue,
            };
        }
        let commit = GitSha::from_str(url.fragment().ok_or(miette::miette!("missing sha"))?)
            .into_diagnostic()?;

        Ok(PinnedGitCheckout {
            commit,
            subdirectory,
            reference,
        })
    }
}

/// A pinned version of a git checkout.
/// Similar with [`GitUrl`] but with a resolved commit field.
#[derive(Debug, Clone)]
pub struct PinnedGitSpec {
    /// The URL of the repository without the revision and subdirectory fragment.
    pub git: Url,
    // The resolved git checkout.
    pub source: PinnedGitCheckout,
}

impl PinnedGitSpec {
    /// Construct the lockfile-compatible [`URL`] from [`PinnedGitSpec`].
    pub fn into_locked_git_url(&self) -> LockedGitUrl {
        let mut url = self.git.clone();

        // // Redact the credentials.
        redact_credentials(&mut url);

        // Clear out any existing state.
        url.set_fragment(None);
        url.set_query(None);

        // Put the subdirectory in the query.
        if let Some(subdirectory) = self.source.subdirectory.as_deref() {
            url.query_pairs_mut()
                .append_pair("subdirectory", subdirectory);
        }

        // Put the requested reference in the query.
        match &self.source.reference {
            Reference::Branch(branch) => {
                url.query_pairs_mut()
                    .append_pair("branch", branch.to_string().as_str());
            }
            Reference::Tag(tag) => {
                url.query_pairs_mut()
                    .append_pair("tag", tag.to_string().as_str());
            }
            Reference::Rev(rev) => {
                url.query_pairs_mut()
                    .append_pair("rev", rev.to_string().as_str());
            }
            Reference::DefaultBranch => {}
        }

        // Put the precise commit in the fragment.
        url.set_fragment(self.source.commit.to_string().as_str().into());

        // prepend git+ to the scheme
        // by transforming it into the string
        // as url does not allow to change from https to git+https.

        let url_str = url.to_string();
        let git_prefix = format!("git+{}", url_str);

        let url = Url::parse(&git_prefix).unwrap();

        LockedGitUrl(url)
    }
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
    fn from(value: PinnedGitSpec) -> Self {
        let url = value.into_locked_git_url();
        UrlOrPath::Url(url.into())
    }
}

impl From<PinnedUrlSpec> for UrlOrPath {
    fn from(_value: PinnedUrlSpec) -> Self {
        unimplemented!()
    }
}

/// A lockfile-compatible [`URL`].
/// The main difference between this and a regular URL
/// is that the fragments contains the precise commit hash and a reference
/// and all credentials are redacted.
/// Also the scheme is prefixed with `git+`.
pub struct LockedGitUrl(Url);

impl LockedGitUrl {
    pub fn is_locked_git_url(locked_url: &Url) -> bool {
        locked_url.scheme().starts_with("git+")
    }

    pub fn to_pinned_git_spec(&self) -> miette::Result<PinnedGitSpec> {
        let git_source = PinnedGitCheckout::from_locked_url(self)?;

        let git_url = GitUrl::try_from(self.0.clone()).into_diagnostic()?;

        // strip git+ from the scheme
        let git_url = git_url.repository().clone();
        let stripped_url = git_url
            .as_str()
            .strip_prefix("git+")
            .unwrap_or(git_url.as_str());
        let stripped_url = Url::parse(stripped_url).unwrap();

        Ok(PinnedGitSpec {
            git: stripped_url,
            source: git_source,
        })
    }
}

impl From<LockedGitUrl> for Url {
    fn from(value: LockedGitUrl) -> Self {
        value.0
    }
}

impl TryFrom<LockedGitUrl> for PinnedGitSpec {
    type Error = miette::Report;
    fn try_from(value: LockedGitUrl) -> Result<Self, Self::Error> {
        value.to_pinned_git_spec()
    }
}

impl From<PinnedGitSpec> for LockedGitUrl {
    fn from(value: PinnedGitSpec) -> Self {
        value.into_locked_git_url()
    }
}

#[derive(Debug, Error)]
pub enum ParseError {}

impl TryFrom<UrlOrPath> for PinnedSourceSpec {
    type Error = ParseError;

    fn try_from(value: UrlOrPath) -> Result<Self, Self::Error> {
        match value {
            UrlOrPath::Url(url) => {
                // for url we can have git+ and simple url's.
                match LockedGitUrl::is_locked_git_url(&url) {
                    true => {
                        let locked_url = LockedGitUrl(url);
                        let pinned = locked_url.to_pinned_git_spec().unwrap();
                        Ok(pinned.into())
                    }
                    false => unimplemented!("url not supported"),
                }
            }
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
        let mut to_be_redacted = spec.git.clone();
        redact_credentials(&mut to_be_redacted);

        if RepositoryUrl::new(&self.git) != RepositoryUrl::new(&to_be_redacted) {
            return Err(SourceMismatchError::UrlMismatch {
                locked: self.git.clone(),
                requested: spec.git.clone(),
            });
        }

        let locked_git_ref = self.source.reference.clone();

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
        write!(f, "{}@{}", self.git, self.source.commit)
    }
}
