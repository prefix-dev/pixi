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

impl Display for GitSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.git)?;
        if let Some(rev) = &self.rev {
            write!(f, " @ {rev}")?;
        }
        if let Some(subdir) = &self.subdirectory {
            write!(f, " in {subdir}")?;
        }
        Ok(())
    }
}

impl GitSpec {
    /// Checks if two `GitSpec` objects are semantically equal.
    ///
    /// This comparison is more lenient than exact equality (`PartialEq`):
    /// - URLs are compared using `RepositoryUrl` which normalizes `.git` suffix and case
    /// - `rev: None` is considered equal to `rev: Some(GitReference::DefaultBranch)`
    /// - Subdirectories are compared exactly
    ///
    /// This is useful for comparing git specs from different sources (e.g., lock file
    /// vs. current metadata) where the same logical reference might be represented
    /// differently.
    pub fn semantically_equal(&self, other: &GitSpec) -> bool {
        use pixi_git::url::RepositoryUrl;

        // Compare repository URLs (ignoring .git suffix, case, etc.)
        if RepositoryUrl::new(&self.git) != RepositoryUrl::new(&other.git) {
            return false;
        }

        // Compare subdirectories exactly
        if self.subdirectory != other.subdirectory {
            return false;
        }

        // Compare revisions semantically
        // `None` and `Some(DefaultBranch)` are considered equivalent
        let self_is_default = GitReference::is_default_branch(&self.rev);
        let other_is_default = GitReference::is_default_branch(&other.rev);

        match (self_is_default, other_is_default) {
            (true, true) => true,
            (false, false) => self.rev == other.rev,
            _ => false,
        }
    }
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
            GitReference::Branch(branch) => write!(f, "{branch}"),
            GitReference::Tag(tag) => write!(f, "{tag}"),
            GitReference::Rev(rev) => write!(f, "{rev}"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    #[test]
    fn test_git_spec_semantically_equal_same_specs() {
        let spec1 = GitSpec {
            git: Url::parse("https://github.com/user/repo").unwrap(),
            rev: Some(GitReference::Branch("main".to_string())),
            subdirectory: None,
        };
        let spec2 = spec1.clone();
        assert!(spec1.semantically_equal(&spec2));
    }

    #[test]
    fn test_git_spec_semantically_equal_url_normalization() {
        // URLs with and without .git suffix should be equal
        let spec1 = GitSpec {
            git: Url::parse("https://github.com/user/repo").unwrap(),
            rev: Some(GitReference::Branch("main".to_string())),
            subdirectory: None,
        };
        let spec2 = GitSpec {
            git: Url::parse("https://github.com/user/repo.git").unwrap(),
            rev: Some(GitReference::Branch("main".to_string())),
            subdirectory: None,
        };
        assert!(spec1.semantically_equal(&spec2));
    }

    #[test]
    fn test_git_spec_semantically_equal_default_branch_vs_none() {
        // None and DefaultBranch should be considered equal
        let spec1 = GitSpec {
            git: Url::parse("https://github.com/user/repo").unwrap(),
            rev: None,
            subdirectory: None,
        };
        let spec2 = GitSpec {
            git: Url::parse("https://github.com/user/repo").unwrap(),
            rev: Some(GitReference::DefaultBranch),
            subdirectory: None,
        };
        assert!(spec1.semantically_equal(&spec2));
    }

    #[test]
    fn test_git_spec_semantically_equal_different_revs() {
        // Different explicit revisions should not be equal
        let spec1 = GitSpec {
            git: Url::parse("https://github.com/user/repo").unwrap(),
            rev: Some(GitReference::Branch("main".to_string())),
            subdirectory: None,
        };
        let spec2 = GitSpec {
            git: Url::parse("https://github.com/user/repo").unwrap(),
            rev: Some(GitReference::Branch("develop".to_string())),
            subdirectory: None,
        };
        assert!(!spec1.semantically_equal(&spec2));
    }

    #[test]
    fn test_git_spec_semantically_equal_different_repos() {
        // Different repositories should not be equal
        let spec1 = GitSpec {
            git: Url::parse("https://github.com/user/repo1").unwrap(),
            rev: Some(GitReference::Branch("main".to_string())),
            subdirectory: None,
        };
        let spec2 = GitSpec {
            git: Url::parse("https://github.com/user/repo2").unwrap(),
            rev: Some(GitReference::Branch("main".to_string())),
            subdirectory: None,
        };
        assert!(!spec1.semantically_equal(&spec2));
    }

    #[test]
    fn test_git_spec_semantically_equal_different_subdirectories() {
        // Different subdirectories should not be equal
        let spec1 = GitSpec {
            git: Url::parse("https://github.com/user/repo").unwrap(),
            rev: Some(GitReference::Branch("main".to_string())),
            subdirectory: Some("dir1".to_string()),
        };
        let spec2 = GitSpec {
            git: Url::parse("https://github.com/user/repo").unwrap(),
            rev: Some(GitReference::Branch("main".to_string())),
            subdirectory: Some("dir2".to_string()),
        };
        assert!(!spec1.semantically_equal(&spec2));
    }

    #[test]
    fn test_git_spec_semantically_equal_rev_vs_branch() {
        // Rev and Branch variants have different semantic meanings even with same name
        let spec1 = GitSpec {
            git: Url::parse("https://github.com/user/repo").unwrap(),
            rev: Some(GitReference::Rev("main".to_string())),
            subdirectory: None,
        };
        let spec2 = GitSpec {
            git: Url::parse("https://github.com/user/repo").unwrap(),
            rev: Some(GitReference::Branch("main".to_string())),
            subdirectory: None,
        };
        // These are different because Rev and Branch are different enum variants
        assert!(!spec1.semantically_equal(&spec2));
    }

    #[test]
    fn test_git_spec_semantically_equal_tag_vs_rev() {
        // Tag and Rev with same name should not be equal
        let spec1 = GitSpec {
            git: Url::parse("https://github.com/user/repo").unwrap(),
            rev: Some(GitReference::Tag("v1.0".to_string())),
            subdirectory: None,
        };
        let spec2 = GitSpec {
            git: Url::parse("https://github.com/user/repo").unwrap(),
            rev: Some(GitReference::Rev("v1.0".to_string())),
            subdirectory: None,
        };
        assert!(!spec1.semantically_equal(&spec2));
    }
}
