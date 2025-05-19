use std::fmt::Display;

use pixi_git::git;
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
    #[serde(skip_serializing_if = "GitReference::is_default_branch", flatten)]
    pub rev: Option<GitReference>,

    /// The git subdirectory of the package
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subdirectory: Option<String>,
}

/// A reference to a specific commit in a git repository.
#[derive(Default, Debug, Clone, Hash, Eq, PartialEq, PartialOrd, Ord, ::serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GitReference {
    /// The HEAD commit of a branch.
    Branch(String),

    /// A specific tag.
    Tag(String),

    /// A specific commit.
    Rev(String),

    /// A default branch.
    #[default]
    DefaultBranch,
}

impl GitReference {
    /// Return the inner value
    pub fn reference(&self) -> Option<String> {
        match self {
            GitReference::Branch(branch) => Some(branch.to_string()),
            GitReference::Tag(tag) => Some(tag.to_string()),
            GitReference::Rev(rev) => Some(rev.to_string()),
            GitReference::DefaultBranch => None,
        }
    }

    /// Return if the reference is the default branch.
    pub fn is_default(&self) -> bool {
        matches!(self, GitReference::DefaultBranch)
    }

    /// Returns the reference as a string.
    pub fn is_default_branch(reference: &Option<GitReference>) -> bool {
        reference.is_none()
            || reference
                .as_ref()
                .is_some_and(|reference| matches!(reference, GitReference::DefaultBranch))
    }

    /// Returns the full commit hash if possible.
    pub fn as_full_commit(&self) -> Option<&str> {
        match self {
            GitReference::Rev(rev) => {
                git::GitReference::looks_like_full_commit_hash(rev).then_some(rev.as_str())
            }
            _ => None,
        }
    }
}

impl Display for GitReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitReference::Branch(branch) => write!(f, "{}", branch),
            GitReference::Tag(tag) => write!(f, "{}", tag),
            GitReference::Rev(rev) => write!(f, "{}", rev),
            GitReference::DefaultBranch => write!(f, "HEAD"),
        }
    }
}

impl From<git::GitReference> for GitReference {
    fn from(value: git::GitReference) -> Self {
        match value {
            git::GitReference::Branch(branch) => GitReference::Branch(branch.to_string()),
            git::GitReference::Tag(tag) => GitReference::Tag(tag.to_string()),
            git::GitReference::ShortCommit(rev) => GitReference::Rev(rev.to_string()),
            git::GitReference::BranchOrTag(rev) => GitReference::Rev(rev.to_string()),
            git::GitReference::BranchOrTagOrCommit(rev) => GitReference::Rev(rev.to_string()),
            git::GitReference::NamedRef(rev) => GitReference::Rev(rev.to_string()),
            git::GitReference::FullCommit(rev) => GitReference::Rev(rev.to_string()),
            git::GitReference::DefaultBranch => GitReference::DefaultBranch,
        }
    }
}

impl Serialize for GitReference {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct RawReference<'a> {
            #[serde(skip_serializing_if = "Option::is_none")]
            tag: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            branch: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            rev: Option<&'a str>,
        }

        let ser = match self {
            GitReference::Branch(name) => RawReference {
                branch: Some(name),
                tag: None,
                rev: None,
            },
            GitReference::Tag(name) => RawReference {
                branch: None,
                tag: Some(name),
                rev: None,
            },
            GitReference::Rev(name) => RawReference {
                branch: None,
                tag: None,
                rev: Some(name),
            },
            GitReference::DefaultBranch => RawReference {
                branch: None,
                tag: None,
                rev: None,
            },
        };

        ser.serialize(serializer)
    }
}

#[derive(Error, Debug)]
/// An error that can occur when converting a `Reference` to a `GitReference`.
pub enum GitReferenceError {
    #[error("The commit string is invalid: \"{0}\"")]
    /// The commit string is invalid.
    InvalidCommit(String),
}

impl From<GitReference> for git::GitReference {
    fn from(value: GitReference) -> Self {
        match value {
            GitReference::Branch(branch) => git::GitReference::Branch(branch),
            GitReference::Tag(tag) => git::GitReference::Tag(tag),
            GitReference::Rev(rev) => git::GitReference::from_rev(rev),
            GitReference::DefaultBranch => git::GitReference::DefaultBranch,
        }
    }
}
