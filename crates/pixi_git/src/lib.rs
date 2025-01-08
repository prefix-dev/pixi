/// Derived from `uv-git` implementation
/// Source: https://github.com/astral-sh/uv/blob/4b8cc3e29e4c2a6417479135beaa9783b05195d3/crates/uv-git/src/lib.rs
/// This module expose types and functions to interact with Git repositories.
use ::url::Url;
use git::GitReference;
use sha::{GitSha, OidParseError};

pub mod credentials;
pub mod git;
pub mod resolver;
pub mod sha;
pub mod source;
pub mod url;

// use resolver::GitResolver;

/// A URL reference to a Git repository.
#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Hash, Ord)]
pub struct GitUrl {
    /// The URL of the Git repository, with any query parameters, fragments removed.
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

    /// Determines if the given URL looks like a Git URL.
    pub fn is_git_url(url: &Url) -> bool {
        // Check if the scheme indicates it's a Git URL.
        if url.scheme().starts_with("git+") {
            // Check if the path ends with .git or has a Git-specific format.
            return true;
        } else if let Some(path) = url.path_segments() {
            if path.clone().any(|segment| segment.ends_with(".git")) {
                return true;
            }
        };

        // If the URL doesn't match Git-specific patterns, return false.
        false
    }
}

impl TryFrom<Url> for GitUrl {
    type Error = OidParseError;

    /// Initialize a [`GitUrl`] source from a URL.
    fn try_from(mut url: Url) -> Result<Self, Self::Error> {
        // Remove any query parameters and fragments.
        url.set_fragment(None);
        url.set_query(None);

        // If the URL ends with a reference, like `https://git.example.com/MyProject.git@v1.0`,
        // extract it.
        let mut reference = GitReference::DefaultBranch;
        if let Some((prefix, suffix)) = url
            .path()
            .rsplit_once('@')
            .map(|(prefix, suffix)| (prefix.to_string(), suffix.to_string()))
        {
            reference = GitReference::from_rev(suffix);
            url.set_path(&prefix);
        }

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
