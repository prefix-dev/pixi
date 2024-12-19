use std::fmt::Display;

use pixi_git::git::GitReference;
use thiserror::Error;
use url::Url;

/// A specification of a package from a git repository.
#[derive(Debug, Clone, Hash, Eq, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct GitSpec {
    /// The git url of the package which can contain git+ prefixes.
    pub git: Url,

    /// The git revision of the package
    #[serde(skip_serializing_if = "Option::is_none", flatten)]
    pub rev: Option<Reference>,

    /// The git subdirectory of the package
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subdirectory: Option<String>,
}

/// A reference to a specific commit in a git repository.
#[derive(
    Debug, Clone, Hash, Eq, PartialEq, PartialOrd, Ord, ::serde::Serialize, ::serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum Reference {
    /// The HEAD commit of a branch.
    Branch(String),

    /// A specific tag.
    Tag(String),

    /// A specific commit.
    Rev(String),

    /// A default branch.
    DefaultBranch,
}

impl Display for Reference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reference::Branch(branch) => write!(f, "{}", branch),
            Reference::Tag(tag) => write!(f, "{}", tag),
            Reference::Rev(rev) => write!(f, "{}", rev),
            Reference::DefaultBranch => write!(f, "HEAD"),
        }
    }
}

impl From<GitReference> for Reference {
    fn from(value: GitReference) -> Self {
        match value {
            GitReference::Branch(branch) => Reference::Branch(branch.to_string()),
            GitReference::Tag(tag) => Reference::Tag(tag.to_string()),
            GitReference::ShortCommit(rev) => Reference::Rev(rev.to_string()),
            GitReference::BranchOrTag(rev) => Reference::Rev(rev.to_string()),
            GitReference::BranchOrTagOrCommit(rev) => Reference::Rev(rev.to_string()),
            GitReference::NamedRef(rev) => Reference::Rev(rev.to_string()),
            GitReference::FullCommit(rev) => Reference::Rev(rev.to_string()),
            GitReference::DefaultBranch => Reference::DefaultBranch,
        }
    }
}

#[derive(Error, Debug)]
pub enum GitReferenceError {
    #[error("The commit string is invalid: \"{0}\"")]
    InvalidCommit(String),
}

impl TryFrom<Reference> for GitReference {
    type Error = GitReferenceError;

    fn try_from(value: Reference) -> Result<Self, Self::Error> {
        match value {
            Reference::Branch(branch) => Ok(GitReference::Branch(branch)),
            Reference::Tag(tag) => Ok(GitReference::Tag(tag)),
            Reference::Rev(rev) => {
                if GitReference::looks_like_commit_hash(&rev) {
                    if rev.len() == 40 {
                        Ok(GitReference::FullCommit(rev))
                    } else {
                        Ok(GitReference::ShortCommit(rev))
                    }
                } else {
                    Err(GitReferenceError::InvalidCommit(rev))
                }
            }
            Reference::DefaultBranch => Ok(GitReference::DefaultBranch),
        }
    }
}
