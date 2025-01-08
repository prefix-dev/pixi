use std::fmt::Display;

use pixi_git::git::GitReference;
use serde::{Serialize, Serializer};
use thiserror::Error;
use url::Url;

/// A specification of a package from a git repository.
#[derive(Debug, Clone, Hash, Eq, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct GitSpec {
    /// The git url of the package which can contain git+ prefixes.
    pub git: Url,

    /// The git revision of the package
    #[serde(skip_serializing_if = "Reference::is_default_branch", flatten)]
    pub rev: Option<Reference>,

    /// The git subdirectory of the package
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subdirectory: Option<String>,
}

/// A reference to a specific commit in a git repository.
#[derive(Debug, Clone, Hash, Eq, PartialEq, PartialOrd, Ord, ::serde::Deserialize)]
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

impl Reference {
    /// Returns the reference as a string.
    pub fn is_default_branch(reference: &Option<Reference>) -> bool {
        reference.is_none()
            || reference
                .as_ref()
                .is_some_and(|reference| matches!(reference, Reference::DefaultBranch))
    }
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

impl Serialize for Reference {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct RawReference<'a> {
            tag: Option<&'a str>,
            branch: Option<&'a str>,
            rev: Option<&'a str>,
        }

        let ser = match self {
            Reference::Branch(name) => RawReference {
                branch: Some(name),
                tag: None,
                rev: None,
            },
            Reference::Tag(name) => RawReference {
                branch: None,
                tag: Some(name),
                rev: None,
            },
            Reference::Rev(name) => RawReference {
                branch: None,
                tag: None,
                rev: Some(name),
            },
            Reference::DefaultBranch => RawReference {
                branch: None,
                tag: None,
                rev: None,
            },
        };

        ser.serialize(serializer)
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
