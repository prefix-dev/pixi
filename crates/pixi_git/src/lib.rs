/// Derived from `uv-git` implementation
/// Source: https://github.com/astral-sh/uv/blob/4b8cc3e29e4c2a6417479135beaa9783b05195d3/crates/uv-git/src/lib.rs
/// This module expose types and functions to interact with Git repositories.
use ::url::Url;
use git::{GitBinaryError, GitReference};
use sha::{GitSha, OidParseError};

pub mod credentials;
pub mod git;
pub mod resolver;
pub mod sha;
pub mod source;
pub mod url;

/// The query parameter used to specify the type of reference in a Git URL.
pub const GIT_URL_QUERY_REV_TYPE: &str = "rev_type";

/// The warning message used in reporter for SSH cloning.
/// This is intended to help user understand that they need to set their SSH passphrase
/// before cloning a repository using SSH, otherwise the process can hang.
/// Original issue: <https://github.com/prefix-dev/pixi/issues/3709>
pub const GIT_SSH_CLONING_WARNING_MSG: &str =
    "Ensure your passphrase is set with ssh-add beforehand to avoid cloning issues.";

/// A URL reference to a Git repository.
#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Hash, Ord)]
pub struct GitUrl {
    /// The URL of the Git repository, with any query parameters, fragments, and leading `git+`
    /// removed.
    repository: Url,
    /// The reference to the commit to use, which could be a branch, tag or revision.
    reference: GitReference,
    /// The precise commit to use, if known.
    precise: Option<GitSha>,
}

impl GitUrl {
    /// Create a new [`GitUrl`] from a repository URL and a reference.
    pub fn from_reference(repository: Url, reference: GitReference) -> Self {
        let precise = reference.as_sha();
        Self {
            repository,
            reference,
            precise,
        }
    }

    /// Create a new [`GitUrl`] from a repository URL and a precise commit.
    pub fn from_commit(repository: Url, reference: GitReference, precise: GitSha) -> Self {
        Self {
            repository,
            reference,
            precise: Some(precise),
        }
    }

    /// Set the precise [`GitSha`] to use for this Git URL.
    #[must_use]
    pub fn with_precise(mut self, precise: GitSha) -> Self {
        self.precise = Some(precise);
        self
    }

    /// Set the [`GitReference`] to use for this Git URL.
    #[must_use]
    pub fn with_reference(mut self, reference: GitReference) -> Self {
        self.reference = reference;
        self
    }

    /// Return the [`Url`] of the Git repository.
    pub fn repository(&self) -> &Url {
        &self.repository
    }

    /// Return the reference to the commit to use, which could be a branch, tag or revision.
    pub fn reference(&self) -> &GitReference {
        &self.reference
    }

    /// Return the precise commit, if known.
    pub fn precise(&self) -> Option<GitSha> {
        self.precise
    }
}

impl TryFrom<Url> for GitUrl {
    type Error = OidParseError;

    /// Initialize a [`GitUrl`] source from a URL.
    fn try_from(url: Url) -> Result<Self, Self::Error> {
        // remove the `git+` prefix if it exists
        let mut url = if url.scheme().starts_with("git+") {
            let url_as_str = &url.as_str()[4..];
            Url::parse(url_as_str).expect("url should be valid")
        } else {
            url
        };

        // Remove any query parameters and fragments.
        url.set_fragment(None);

        // If the URL ends with a reference, like `https://git.example.com/MyProject.git@v1.0`,
        // extract it.
        // The URL can also be enriched with the reference type, like `https://git.example.com/MyProject.git@v1.0?rev_type=tag`.
        // so we can extract the reference and the reference type.
        let mut reference = GitReference::DefaultBranch;
        if let Some((prefix, suffix)) = url
            .path()
            .rsplit_once('@')
            .map(|(prefix, suffix)| (prefix.to_string(), suffix.to_string()))
        {
            if let Some((_, rev_type)) = url
                .query_pairs()
                .find(|(key, _)| key == GIT_URL_QUERY_REV_TYPE)
            {
                match rev_type.into_owned().as_str() {
                    "tag" => reference = GitReference::Tag(suffix),
                    "branch" => reference = GitReference::Branch(suffix),
                    "rev" => reference = GitReference::from_rev(suffix),
                    // a custom reference type is not supported
                    _ => return Err(OidParseError::UrlParse(url.to_string())),
                }
            } else {
                // try to guess it
                reference = GitReference::from_rev(suffix);
            }

            url.set_path(&prefix);
        }
        url.set_query(None);

        Ok(Self::from_reference(url, reference))
    }
}

impl From<GitUrl> for Url {
    fn from(git: GitUrl) -> Self {
        let mut url = git.repository;

        // If we have a precise commit, add `@` and the commit hash to the URL.
        if let Some(precise) = git.precise {
            url.set_path(&format!("{}@{}", url.path(), precise));
        } else {
            // Otherwise, add the branch or tag name.
            match git.reference {
                GitReference::Branch(rev)
                | GitReference::Tag(rev)
                | GitReference::ShortCommit(rev)
                | GitReference::BranchOrTag(rev)
                | GitReference::NamedRef(rev)
                | GitReference::FullCommit(rev)
                | GitReference::BranchOrTagOrCommit(rev) => {
                    url.set_path(&format!("{}@{}", url.path(), rev));
                }
                GitReference::DefaultBranch => {}
            }
        }

        url
    }
}

impl std::fmt::Display for GitUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.repository)
    }
}

pub trait Reporter: Send + Sync {
    /// Callback to invoke when a repository checkout begins.
    fn on_checkout_start(&self, url: &Url, rev: &str) -> usize;

    /// Callback to invoke when a repository checkout completes.
    fn on_checkout_complete(&self, url: &Url, rev: &str, index: usize);
}

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error(transparent)]
    GitBinary(#[from] GitBinaryError),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    FromUtf8(#[from] std::string::FromUtf8Error),

    #[error(transparent)]
    OidParse(#[from] OidParseError),

    #[error("failed to fetch {0}: {1}")]
    Fetch(String, String),

    #[error(transparent)]
    UrlParse(#[from] ::url::ParseError),

    #[error("could not transform original url {0} into a git url: {1}")]
    GitUrlFormat(String, String),

    #[error(transparent)]
    ReqwestMiddleware(#[from] reqwest_middleware::Error),

    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),

    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
}
