#![deny(missing_docs)]
use std::{
    fmt::{Display, Formatter},
    path::{Path, PathBuf},
    str::FromStr,
};

use miette::IntoDiagnostic;
use pixi_git::{
    GitUrl,
    sha::GitSha,
    url::{RepositoryUrl, redact_credentials},
};
use pixi_spec::{
    GitReference, GitSpec, PathSourceSpec, SourceLocationSpec, SourceSpec, UrlSourceSpec,
};
use rattler_digest::{Md5Hash, Sha256Hash};
use rattler_lock::UrlOrPath;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use thiserror::Error;
use typed_path::Utf8TypedPathBuf;
use url::Url;

/// Describes an exact revision of a source checkout. This is used to pin a
/// particular source definition to a revision. A git source spec does not
/// describe an exact commit. This struct describes an exact commit.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PinnedSourceSpec {
    /// A pinned url source package.
    Url(PinnedUrlSpec),
    /// A pinned git source package.
    Git(PinnedGitSpec),
    /// A pinned path source package.
    Path(PinnedPathSpec),
}

/// Describes a mutable source spec. This is similar to a [`PinnedSourceSpec`]
/// but the contents can change over time.
#[derive(Debug, Clone)]
pub enum MutablePinnedSourceSpec {
    /// A mutable path source package.
    Path(PinnedPathSpec),
}

impl PinnedSourceSpec {
    /// Returns the path spec if this instance is a path spec.
    pub fn as_path(&self) -> Option<&PinnedPathSpec> {
        match self {
            PinnedSourceSpec::Path(spec) => Some(spec),
            _ => None,
        }
    }

    /// Returns the url spec if this instance is a url spec.
    pub fn as_url(&self) -> Option<&PinnedUrlSpec> {
        match self {
            PinnedSourceSpec::Url(spec) => Some(spec),
            _ => None,
        }
    }

    /// Returns the git spec if this instance is a git spec.
    pub fn as_git(&self) -> Option<&PinnedGitSpec> {
        match self {
            PinnedSourceSpec::Git(spec) => Some(spec),
            _ => None,
        }
    }

    /// Converts this instance into a [`PinnedPathSpec`] if it is a path spec.
    pub fn into_path(self) -> Option<PinnedPathSpec> {
        match self {
            PinnedSourceSpec::Path(spec) => Some(spec),
            _ => None,
        }
    }

    /// Converts this instance into a [`PinnedUrlSpec`] if it is a url spec.
    pub fn into_url(self) -> Option<PinnedUrlSpec> {
        match self {
            PinnedSourceSpec::Url(spec) => Some(spec),
            _ => None,
        }
    }

    /// Converts this instance into a [`PinnedGitSpec`] if it is a git spec.
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

    /// Returns true if the pinned source may change even if the pinned source
    /// itself does not. This indicates that the contents of this instance may
    /// change over time, such as a local path.
    pub fn is_mutable(&self) -> bool {
        matches!(self, PinnedSourceSpec::Path(_))
    }

    /// Returns a URL that uniquely identifies this path spec. This URL is not
    /// portable, e.g. it might result in a different URL on different systems.
    pub fn identifiable_url(&self) -> Url {
        match self {
            PinnedSourceSpec::Url(spec) => spec.identifiable_url(),
            PinnedSourceSpec::Git(spec) => spec.identifiable_url(),
            PinnedSourceSpec::Path(spec) => spec.identifiable_url(),
        }
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
#[serde_as]
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct PinnedUrlSpec {
    /// The URL of the archive.
    pub url: Url,
    /// The sha256 hash of the archive.
    #[serde_as(as = "rattler_digest::serde::SerializableHash<rattler_digest::Sha256>")]
    pub sha256: Sha256Hash,
    /// The md5 hash of the archive.
    #[serde_as(as = "Option<rattler_digest::serde::SerializableHash<rattler_digest::Md5>>")]
    pub md5: Option<Md5Hash>,
}

impl PinnedUrlSpec {
    /// Returns a URL that uniquely identifies this path spec. This URL is not
    /// portable, e.g. it might result in a different URL on different systems.
    pub fn identifiable_url(&self) -> Url {
        let mut url = self.url.clone();
        url.query_pairs_mut()
            .append_pair("sha256", &format!("{:x}", self.sha256));
        url
    }
}

impl From<PinnedUrlSpec> for PinnedSourceSpec {
    fn from(value: PinnedUrlSpec) -> Self {
        PinnedSourceSpec::Url(value)
    }
}

/// A pinned version of a git checkout.
#[derive(Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PinnedGitCheckout {
    /// The commit hash of the git checkout.
    pub commit: GitSha,
    /// The subdirectory of the git checkout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subdirectory: Option<String>,
    /// The reference of the git checkout.
    #[serde(skip_serializing_if = "GitReference::is_default")]
    pub reference: GitReference,
}

impl PinnedGitCheckout {
    /// Creates a new pinned git checkout.
    pub fn new(commit: GitSha, subdirectory: Option<String>, reference: GitReference) -> Self {
        Self {
            commit,
            subdirectory,
            reference,
        }
    }

    /// Extracts a pinned git checkout from the query pairs and the hash
    /// fragment in the given URL.
    pub fn from_locked_url(locked_url: &LockedGitUrl) -> miette::Result<PinnedGitCheckout> {
        let url = &locked_url.0;
        let mut reference = None;
        let mut subdirectory = None;

        for (key, val) in url.query_pairs() {
            match &*key {
                "tag" => {
                    if reference
                        .replace(GitReference::Tag(val.into_owned()))
                        .is_some()
                    {
                        return Err(miette::miette!("multiple tags in URL"));
                    }
                }
                "branch" => {
                    if reference
                        .replace(GitReference::Branch(val.into_owned()))
                        .is_some()
                    {
                        return Err(miette::miette!("multiple branches in URL"));
                    }
                }
                "rev" => {
                    if reference
                        .replace(GitReference::Rev(val.into_owned()))
                        .is_some()
                    {
                        return Err(miette::miette!("multiple revs in URL"));
                    }
                }
                // If the URL points to a subdirectory, extract it, as in (git):
                //   `git+https://git.example.com/MyProject.git@v1.0#subdirectory=pkg_dir`
                //   `git+https://git.example.com/MyProject.git@v1.0#egg=pkg&subdirectory=pkg_dir`
                "subdirectory" => {
                    if subdirectory.replace(val.into_owned()).is_some() {
                        return Err(miette::miette!("multiple subdirectories in URL"));
                    }
                }
                _ => continue,
            };
        }

        // set the default reference if none is provided.
        if reference.is_none() {
            reference.replace(GitReference::DefaultBranch);
        }

        let commit = GitSha::from_str(url.fragment().ok_or(miette::miette!("missing sha"))?)
            .into_diagnostic()?;

        Ok(PinnedGitCheckout {
            commit,
            subdirectory,
            reference: reference.expect("reference should be set"),
        })
    }
}

/// A pinned version of a git checkout.
/// Similar with [`GitUrl`] but with a resolved commit field.
#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize, Deserialize)]
pub struct PinnedGitSpec {
    /// The URL of the repository without the revision and subdirectory
    /// fragment.
    pub git: Url,
    /// The resolved git checkout.
    #[serde(flatten)]
    pub source: PinnedGitCheckout,
}

impl PinnedGitSpec {
    /// Creates a new pinned git spec.
    pub fn new(git: Url, source: PinnedGitCheckout) -> Self {
        Self { git, source }
    }

    /// Returns a URL that uniquely identifies this path spec. This URL is not
    /// portable, e.g. it might result in a different URL on different systems.
    pub fn identifiable_url(&self) -> Url {
        self.into_locked_git_url().to_url()
    }

    /// Construct the lockfile-compatible [`Url`] from [`PinnedGitSpec`].
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
            GitReference::Branch(branch) => {
                url.query_pairs_mut()
                    .append_pair("branch", branch.to_string().as_str());
            }
            GitReference::Tag(tag) => {
                url.query_pairs_mut()
                    .append_pair("tag", tag.to_string().as_str());
            }
            GitReference::Rev(rev) => {
                url.query_pairs_mut()
                    .append_pair("rev", rev.to_string().as_str());
            }
            GitReference::DefaultBranch => {}
        }

        // Put the precise commit in the fragment.
        url.set_fragment(self.source.commit.to_string().as_str().into());

        // prepend git+ to the scheme
        // by transforming it into the string
        // as url does not allow to change from https to git+https.
        // TODO: this is a good place to type the url.
        let url = if !url.scheme().starts_with("git+") {
            let url_str = url.to_string();

            let git_prefix = format!("git+{}", url_str);

            Url::parse(&git_prefix).expect("url should be valid")
        } else {
            url
        };

        LockedGitUrl(url)
    }
}

impl From<PinnedGitSpec> for PinnedSourceSpec {
    fn from(value: PinnedGitSpec) -> Self {
        PinnedSourceSpec::Git(value)
    }
}

/// A pinned version of a path based source dependency. Different from a
/// `PathSpec` this path is always either absolute or relative to the project
/// root.
#[serde_as]
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct PinnedPathSpec {
    /// The path of the source.
    #[serde_as(
        serialize_as = "serde_with::DisplayFromStr",
        deserialize_as = "serde_with::FromInto<String>"
    )]
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

    /// Returns a URL that uniquely identifies this path spec. This URL is not
    /// portable, e.g. it might result in a different URL on different systems.
    pub fn identifiable_url(&self) -> Url {
        let resolved = if cfg!(windows) {
            self.resolve(Path::new("\\\\localhost\\"))
        } else {
            self.resolve(Path::new("/"))
        };
        Url::from_directory_path(resolved).expect("expected valid URL")
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

/// A lockfile-compatible [`Url`].
/// The main difference between this and a regular URL
/// is that the fragments contains the precise commit hash and a reference
/// and all credentials are redacted.
/// Also the scheme is prefixed with `git+`.
///
/// # Examples
///
/// ```
/// use pixi_record::LockedGitUrl;
/// let locked_url = LockedGitUrl::parse("git+https://github.com/nichmor/pixi-build-examples?branch=fix-backend#1c4b2c7864a60ea169e091901fcde63a8d6fbfdc").unwrap();
/// ```
pub struct LockedGitUrl(Url);

impl LockedGitUrl {
    /// Creates a new [`LockedGitUrl`] from a [`Url`].
    pub fn new(url: Url) -> Self {
        Self(url)
    }

    /// Returns true if the given URL is a locked git URL.
    /// This is used to differentiate between a regular Url and a
    /// [`LockedGitUrl`] that starts with `git+`.
    pub fn is_locked_git_url(locked_url: &Url) -> bool {
        locked_url.scheme().starts_with("git+")
    }

    /// Converts this [`LockedGitUrl`] into a [`PinnedGitSpec`].
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

    /// Parses a locked git URL from a string.
    pub fn parse(url: &str) -> miette::Result<Self> {
        let url = Url::parse(url).into_diagnostic()?;
        Ok(Self(url))
    }

    /// Converts this [`LockedGitUrl`] into a [`Url`].
    pub fn to_url(&self) -> Url {
        self.0.clone()
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
/// An error that occurs when parsing a [`PinnedSourceSpec`].
pub enum ParseError {
    /// An error that occurs when parsing a locked git URL.
    #[error("failed to parse locked git url {0}. Reason: {1}")]
    LockedGitUrl(String, String),
}

impl TryFrom<UrlOrPath> for PinnedSourceSpec {
    type Error = ParseError;

    fn try_from(value: UrlOrPath) -> Result<Self, Self::Error> {
        match value {
            UrlOrPath::Url(url) => {
                // for url we can have git+ and simple url's.
                match LockedGitUrl::is_locked_git_url(&url) {
                    true => {
                        let locked_url = LockedGitUrl(url.clone());
                        let pinned = locked_url.to_pinned_git_spec().map_err(|err| {
                            ParseError::LockedGitUrl(url.to_string(), err.to_string())
                        })?;
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
/// An error that occurs when verifying if lock file satisfy requirements.
pub enum SourceMismatchError {
    #[error("the locked path '{locked}' does not match the requested path '{requested}'")]
    /// The locked path does not match the requested path.
    PathMismatch {
        /// The locked path.
        locked: Utf8TypedPathBuf,
        /// The requested path.
        requested: Utf8TypedPathBuf,
    },

    #[error("the locked url '{locked}' does not match the requested url '{requested}'")]
    /// The locked url does not match the requested url.
    UrlMismatch {
        /// The locked url.
        locked: Url,
        /// The requested url.
        requested: Url,
    },

    #[error(
        "the locked {hash} of url '{url}' ({locked}) does not match the requested {hash} ({requested})"
    )]
    /// The locked hash of the url does not match the requested hash.
    UrlHashMismatch {
        /// The hash
        hash: &'static str,
        /// The url.
        url: Url,
        /// The locked url
        locked: String,
        /// The requested url
        requested: String,
    },

    #[error(
        "the locked git rev '{locked}' for '{git}' does not match the requested git rev '{requested}'"
    )]
    /// The locked git rev does not match the requested git rev.
    GitRevMismatch {
        /// The git url.
        git: Url,
        /// The locked git rev.
        locked: String,
        /// The requested git rev.
        requested: String,
    },

    #[error(
        "the locked git subdirectory '{locked:?}' for '{git}' does not match the requested git subdirectory '{requested:?}'"
    )]
    /// The locked git rev does not match the requested git rev.
    GitSubdirectoryMismatch {
        /// The git url.
        git: Url,
        /// The locked git subdirectory.
        locked: Option<String>,
        /// The requested git subdirectory.
        requested: Option<String>,
    },

    #[error("the locked source type does not match the requested type")]
    /// The locked source type does not match the requested type.
    SourceTypeMismatch,
}

impl PinnedPathSpec {
    #[allow(clippy::result_large_err)]
    /// Verifies if the locked path satisfies the requested path.
    pub fn satisfies(&self, spec: &PathSourceSpec) -> Result<(), SourceMismatchError> {
        if spec.path.normalize() != self.path.normalize() {
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
    /// Verifies if the locked url satisfies the requested url.
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
    /// Verifies if the locked git url satisfies the requested git url.
    pub fn satisfies(&self, spec: &GitSpec) -> Result<(), SourceMismatchError> {
        let mut to_be_redacted = spec.git.clone();
        redact_credentials(&mut to_be_redacted);

        if RepositoryUrl::new(&self.git) != RepositoryUrl::new(&to_be_redacted) {
            return Err(SourceMismatchError::UrlMismatch {
                locked: self.git.clone(),
                requested: spec.git.clone(),
            });
        }

        // Check if the subdirectory matches.
        if self.source.subdirectory != spec.subdirectory {
            return Err(SourceMismatchError::GitSubdirectoryMismatch {
                git: self.git.clone(),
                locked: self.source.subdirectory.clone(),
                requested: spec.subdirectory.clone(),
            });
        }

        // Check if requested rev matches.
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
    /// Verifies if the locked source satisfies the requested source.
    pub fn satisfies(&self, spec: &SourceSpec) -> Result<(), SourceMismatchError> {
        match (self, &spec.location) {
            (PinnedSourceSpec::Path(locked), SourceLocationSpec::Path(spec)) => {
                locked.satisfies(spec)
            }
            (PinnedSourceSpec::Url(locked), SourceLocationSpec::Url(spec)) => {
                locked.satisfies(spec)
            }
            (PinnedSourceSpec::Git(locked), SourceLocationSpec::Git(spec)) => {
                locked.satisfies(spec)
            }
            (_, _) => Err(SourceMismatchError::SourceTypeMismatch),
        }
    }
}

impl Display for PinnedSourceSpec {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            PinnedSourceSpec::Path(spec) => write!(f, "{}", spec.path),
            PinnedSourceSpec::Url(spec) => write!(f, "{}", spec.url),
            PinnedSourceSpec::Git(spec) => write!(f, "{}@{}", spec.git, spec.source.commit),
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

impl From<PinnedSourceSpec> for SourceSpec {
    fn from(value: PinnedSourceSpec) -> Self {
        match value {
            PinnedSourceSpec::Url(url) => SourceSpec {
                location: SourceLocationSpec::Url(url.into()),
            },
            PinnedSourceSpec::Git(git) => SourceSpec {
                location: SourceLocationSpec::Git(git.into()),
            },
            PinnedSourceSpec::Path(path) => SourceSpec {
                location: SourceLocationSpec::Path(path.into()),
            },
        }
    }
}

impl From<PinnedPathSpec> for PathSourceSpec {
    fn from(value: PinnedPathSpec) -> Self {
        Self { path: value.path }
    }
}

impl From<PinnedUrlSpec> for UrlSourceSpec {
    fn from(value: PinnedUrlSpec) -> Self {
        Self {
            url: value.url,
            sha256: Some(value.sha256),
            md5: value.md5,
        }
    }
}

impl From<PinnedGitSpec> for GitSpec {
    fn from(value: PinnedGitSpec) -> Self {
        Self {
            git: value.git,
            subdirectory: value.source.subdirectory,
            rev: Some(value.source.reference),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use pixi_git::sha::GitSha;
    use pixi_spec::{GitReference, GitSpec};
    use url::Url;

    use crate::{PinnedGitCheckout, PinnedGitSpec, SourceMismatchError};

    #[test]
    fn test_spec_satisfies() {
        let locked_git_spec = PinnedGitSpec {
            git: Url::parse("https://github.com/example/repo.git").unwrap(),
            source: PinnedGitCheckout {
                commit: GitSha::from_str("9de9e1b48cc421f05fc6aa6918cade3033a38c32").unwrap(),
                subdirectory: None,
                reference: pixi_spec::GitReference::Rev("9de9e1b".to_string()),
            },
        };

        let requested_git_spec = GitSpec {
            git: Url::parse("https://github.com/example/repo.git").unwrap(),
            subdirectory: None,
            rev: Some(pixi_spec::GitReference::Rev("9de9e1b".to_string())),
        };

        let result = locked_git_spec.satisfies(&requested_git_spec);

        assert!(result.is_ok());

        let locked_git_spec_without_git_suffix = PinnedGitSpec {
            git: Url::parse("https://github.com/example/repo").unwrap(),
            source: PinnedGitCheckout {
                commit: GitSha::from_str("9de9e1b48cc421f05fc6aa6918cade3033a38c32").unwrap(),
                subdirectory: None,
                reference: pixi_spec::GitReference::Rev("9de9e1b".to_string()),
            },
        };

        let requested_git_spec = GitSpec {
            git: Url::parse("https://github.com/example/repo.git").unwrap(),
            subdirectory: None,
            rev: Some(pixi_spec::GitReference::Rev("9de9e1b".to_string())),
        };

        let result = locked_git_spec_without_git_suffix.satisfies(&requested_git_spec);

        assert!(result.is_ok());

        let locked_git_spec = PinnedGitSpec {
            git: Url::parse("https://github.com/example/repo.git").unwrap(),
            source: PinnedGitCheckout {
                commit: GitSha::from_str("9de9e1b48cc421f05fc6aa6918cade3033a38c32").unwrap(),
                subdirectory: None,
                reference: pixi_spec::GitReference::Rev("9de9e1b".to_string()),
            },
        };

        let requested_git_spec_without_suffix = GitSpec {
            git: Url::parse("https://github.com/example/repo").unwrap(),
            subdirectory: None,
            rev: Some(pixi_spec::GitReference::Rev("9de9e1b".to_string())),
        };

        let result = locked_git_spec.satisfies(&requested_git_spec_without_suffix);

        assert!(result.is_ok());

        let locked_git_spec = PinnedGitSpec {
            git: Url::parse("https://username:password@github.com/example/repo").unwrap(),
            source: PinnedGitCheckout {
                commit: GitSha::from_str("9de9e1b48cc421f05fc6aa6918cade3033a38c32").unwrap(),
                subdirectory: None,
                reference: pixi_spec::GitReference::Rev("9de9e1b".to_string()),
            },
        };

        let requested_git_spec = GitSpec {
            git: Url::parse("https://github.com/example/repo.git").unwrap(),
            subdirectory: None,
            rev: Some(pixi_spec::GitReference::Rev("9de9e1b".to_string())),
        };

        let result = locked_git_spec.satisfies(&requested_git_spec);

        assert!(result.is_ok());

        let locked_git_spec = PinnedGitSpec {
            git: Url::parse("https://username:password@github.com/example/repo").unwrap(),
            source: PinnedGitCheckout {
                commit: GitSha::from_str("9de9e1b48cc421f05fc6aa6918cade3033a38c32").unwrap(),
                subdirectory: None,
                reference: pixi_spec::GitReference::Rev("9de9e1b".to_string()),
            },
        };

        let requested_git_spec_with_prefix = GitSpec {
            git: Url::parse("git+https://github.com/example/repo.git").unwrap(),
            subdirectory: None,
            rev: Some(pixi_spec::GitReference::Rev("9de9e1b".to_string())),
        };

        let result = locked_git_spec.satisfies(&requested_git_spec_with_prefix);

        result.unwrap();

        // assert!(result.is_ok());
    }

    #[test]
    fn test_rev_is_different() {
        let locked_git_spec = PinnedGitSpec {
            git: Url::parse("https://github.com/example/repo.git").unwrap(),
            source: PinnedGitCheckout {
                commit: GitSha::from_str("9de9e1b48cc421f05fc6aa6918cade3033a38c32").unwrap(),
                subdirectory: None,
                reference: pixi_spec::GitReference::Rev("9de9e1b".to_string()),
            },
        };

        let requested_git_spec = GitSpec {
            git: Url::parse("https://github.com/example/repo.git").unwrap(),
            subdirectory: None,
            rev: Some(pixi_spec::GitReference::Rev("d2e32".to_string())),
        };

        let result = locked_git_spec.satisfies(&requested_git_spec).unwrap_err();
        assert!(matches!(result, SourceMismatchError::GitRevMismatch { .. }));
    }

    #[test]
    fn test_url_mismatch() {
        let locked_git_spec = PinnedGitSpec {
            git: Url::parse("https://github.com/another-example/repo.git").unwrap(),
            source: PinnedGitCheckout {
                commit: GitSha::from_str("9de9e1b48cc421f05fc6aa6918cade3033a38c32").unwrap(),
                subdirectory: None,
                reference: pixi_spec::GitReference::Rev("9de9e1b".to_string()),
            },
        };

        let requested_git_spec = GitSpec {
            git: Url::parse("https://github.com/example/repo.git").unwrap(),
            subdirectory: None,
            rev: Some(pixi_spec::GitReference::Rev("9de9e1b".to_string())),
        };

        let result = locked_git_spec.satisfies(&requested_git_spec).unwrap_err();
        assert!(matches!(result, SourceMismatchError::UrlMismatch { .. }));
    }

    #[test]
    fn test_default_branch() {
        let locked_git_spec = PinnedGitSpec {
            git: Url::parse("https://github.com/example/repo.git").unwrap(),
            source: PinnedGitCheckout {
                commit: GitSha::from_str("9de9e1b48cc421f05fc6aa6918cade3033a38c32").unwrap(),
                subdirectory: None,
                reference: GitReference::DefaultBranch,
            },
        };

        let requested_git_spec = GitSpec {
            git: Url::parse("https://github.com/example/repo.git").unwrap(),
            subdirectory: None,
            // we are not specifying the rev
            // and request the default branch
            rev: None,
        };

        let result = locked_git_spec.satisfies(&requested_git_spec);
        assert!(result.is_ok());
    }

    #[test]
    fn test_requesting_subdirectory() {
        let locked_git_spec = PinnedGitSpec {
            git: Url::parse("https://github.com/example/repo.git").unwrap(),
            source: PinnedGitCheckout {
                commit: GitSha::from_str("9de9e1b48cc421f05fc6aa6918cade3033a38c32").unwrap(),
                subdirectory: Some("some-subdir".to_string()),
                reference: GitReference::DefaultBranch,
            },
        };

        let requested_git_spec = GitSpec {
            git: Url::parse("https://github.com/example/repo.git").unwrap(),
            subdirectory: None,
            // we are not specifying the rev
            // and request the default branch
            rev: None,
        };

        let result = locked_git_spec.satisfies(&requested_git_spec).unwrap_err();
        assert!(matches!(
            result,
            SourceMismatchError::GitSubdirectoryMismatch { .. }
        ));

        // check when we dont lock subdirectory, but request it
        let locked_git_spec = PinnedGitSpec {
            git: Url::parse("https://github.com/example/repo.git").unwrap(),
            source: PinnedGitCheckout {
                commit: GitSha::from_str("9de9e1b48cc421f05fc6aa6918cade3033a38c32").unwrap(),
                subdirectory: None,
                reference: GitReference::DefaultBranch,
            },
        };

        let requested_git_spec = GitSpec {
            git: Url::parse("https://github.com/example/repo.git").unwrap(),
            subdirectory: Some("some-subdir".to_string()),
            // we are not specifying the rev
            // and request the default branch
            rev: None,
        };

        let result = locked_git_spec.satisfies(&requested_git_spec).unwrap_err();
        assert!(matches!(
            result,
            SourceMismatchError::GitSubdirectoryMismatch { .. }
        ));
    }
}
