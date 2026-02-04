//! Canonical source specifications for equality comparison.
//!
//! This module provides types that represent the canonical form of source
//! specifications, where only the truly identifying information is retained.
//! For git sources, this means using only the commit hash rather than
//! branch/tag/rev references.

use std::fmt::{Display, Formatter};

use pixi_git::{sha::GitSha, url::RepositoryUrl};
use pixi_spec::Subdirectory;
use rattler_digest::Sha256Hash;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use typed_path::Utf8TypedPathBuf;
use url::Url;

use crate::{PinnedGitSpec, PinnedPathSpec, PinnedSourceSpec, PinnedUrlSpec};

/// A canonical representation of a source specification used for equality
/// comparison.
///
/// Unlike [`PinnedSourceSpec`], this type only contains the truly identifying
/// information. For git sources, this means only the commit hash is used for
/// comparison, not the branch/tag/rev reference.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CanonicalSpec {
    /// A canonical url source.
    Url(CanonicalUrlSpec),
    /// A canonical git source.
    Git(CanonicalGitSpec),
    /// A canonical path source.
    Path(CanonicalPathSpec),
}

/// A canonical representation of a URL source.
#[serde_as]
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct CanonicalUrlSpec {
    /// The URL of the archive.
    pub url: Url,
    /// The sha256 hash of the archive.
    #[serde_as(as = "rattler_digest::serde::SerializableHash<rattler_digest::Sha256>")]
    pub sha256: Sha256Hash,
    /// The subdirectory within the archive.
    #[serde(skip_serializing_if = "Subdirectory::is_empty", default)]
    pub subdirectory: Subdirectory,
}

/// A canonical representation of a git source.
///
/// This only contains the repository URL (normalized), commit hash, and
/// subdirectory. The git reference (branch/tag/rev) is deliberately excluded
/// as it doesn't affect the actual content.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct CanonicalGitSpec {
    /// The normalized repository URL.
    pub repository: RepositoryUrl,
    /// The commit hash.
    pub commit: GitSha,
    /// The subdirectory within the repository.
    #[serde(skip_serializing_if = "Subdirectory::is_empty", default)]
    pub subdirectory: Subdirectory,
}

/// A canonical representation of a path source.
#[serde_as]
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct CanonicalPathSpec {
    /// The path to the source.
    #[serde_as(
        serialize_as = "serde_with::DisplayFromStr",
        deserialize_as = "serde_with::FromInto<String>"
    )]
    pub path: Utf8TypedPathBuf,
}

impl From<&PinnedSourceSpec> for CanonicalSpec {
    fn from(spec: &PinnedSourceSpec) -> Self {
        match spec {
            PinnedSourceSpec::Url(url) => CanonicalSpec::Url(url.into()),
            PinnedSourceSpec::Git(git) => CanonicalSpec::Git(git.into()),
            PinnedSourceSpec::Path(path) => CanonicalSpec::Path(path.into()),
        }
    }
}

impl From<PinnedSourceSpec> for CanonicalSpec {
    fn from(spec: PinnedSourceSpec) -> Self {
        CanonicalSpec::from(&spec)
    }
}

impl From<&PinnedUrlSpec> for CanonicalUrlSpec {
    fn from(spec: &PinnedUrlSpec) -> Self {
        Self {
            url: spec.url.clone(),
            sha256: spec.sha256,
            subdirectory: spec.subdirectory.clone(),
        }
    }
}

impl From<&PinnedGitSpec> for CanonicalGitSpec {
    fn from(spec: &PinnedGitSpec) -> Self {
        Self {
            repository: RepositoryUrl::new(&spec.git),
            commit: spec.source.commit,
            subdirectory: spec.source.subdirectory.clone(),
        }
    }
}

impl From<&PinnedPathSpec> for CanonicalPathSpec {
    fn from(spec: &PinnedPathSpec) -> Self {
        Self {
            path: spec.path.clone(),
        }
    }
}

impl Display for CanonicalSpec {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            CanonicalSpec::Url(spec) => Display::fmt(spec, f),
            CanonicalSpec::Git(spec) => Display::fmt(spec, f),
            CanonicalSpec::Path(spec) => Display::fmt(spec, f),
        }
    }
}

impl Display for CanonicalUrlSpec {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut url = self.url.clone();
        url.query_pairs_mut()
            .append_pair("sha256", &format!("{:x}", self.sha256));
        if !self.subdirectory.is_empty() {
            url.query_pairs_mut()
                .append_pair("subdirectory", &self.subdirectory.to_string());
        }
        Display::fmt(&url, f)
    }
}

impl Display for CanonicalGitSpec {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut url = self.repository.as_url().clone();
        if !self.subdirectory.is_empty() {
            url.query_pairs_mut()
                .append_pair("subdirectory", &self.subdirectory.to_string());
        }
        url.set_fragment(Some(&self.commit.to_string()));
        Display::fmt(&url, f)
    }
}

impl Display for CanonicalPathSpec {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.path, f)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use pixi_git::sha::GitSha;
    use pixi_spec::{GitReference, Subdirectory};
    use url::Url;

    use crate::{PinnedGitCheckout, PinnedGitSpec, PinnedSourceSpec};

    use super::CanonicalSpec;

    #[test]
    fn test_git_same_commit_different_reference() {
        // Two git specs with the same commit but different references should be equal
        let spec1 = PinnedSourceSpec::Git(PinnedGitSpec {
            git: Url::parse("https://github.com/user/repo").unwrap(),
            source: PinnedGitCheckout {
                commit: GitSha::from_str("abc123def456789012345678901234567890abcd").unwrap(),
                subdirectory: Default::default(),
                reference: GitReference::Branch("main".to_string()),
            },
        });

        let spec2 = PinnedSourceSpec::Git(PinnedGitSpec {
            git: Url::parse("https://github.com/user/repo").unwrap(),
            source: PinnedGitCheckout {
                commit: GitSha::from_str("abc123def456789012345678901234567890abcd").unwrap(),
                subdirectory: Default::default(),
                reference: GitReference::Tag("v1.0.0".to_string()),
            },
        });

        let canonical1 = CanonicalSpec::from(&spec1);
        let canonical2 = CanonicalSpec::from(&spec2);

        assert_eq!(canonical1, canonical2);
    }

    #[test]
    fn test_git_different_commits() {
        let spec1 = PinnedSourceSpec::Git(PinnedGitSpec {
            git: Url::parse("https://github.com/user/repo").unwrap(),
            source: PinnedGitCheckout {
                commit: GitSha::from_str("abc123def456789012345678901234567890abcd").unwrap(),
                subdirectory: Default::default(),
                reference: GitReference::DefaultBranch,
            },
        });

        let spec2 = PinnedSourceSpec::Git(PinnedGitSpec {
            git: Url::parse("https://github.com/user/repo").unwrap(),
            source: PinnedGitCheckout {
                commit: GitSha::from_str("def456789012345678901234567890abcdabc123").unwrap(),
                subdirectory: Default::default(),
                reference: GitReference::DefaultBranch,
            },
        });

        let canonical1 = CanonicalSpec::from(&spec1);
        let canonical2 = CanonicalSpec::from(&spec2);

        assert_ne!(canonical1, canonical2);
    }

    #[test]
    fn test_git_url_normalization() {
        // URLs with/without .git suffix should be equal
        let spec1 = PinnedSourceSpec::Git(PinnedGitSpec {
            git: Url::parse("https://github.com/user/repo").unwrap(),
            source: PinnedGitCheckout {
                commit: GitSha::from_str("abc123def456789012345678901234567890abcd").unwrap(),
                subdirectory: Default::default(),
                reference: GitReference::DefaultBranch,
            },
        });

        let spec2 = PinnedSourceSpec::Git(PinnedGitSpec {
            git: Url::parse("https://github.com/user/repo.git").unwrap(),
            source: PinnedGitCheckout {
                commit: GitSha::from_str("abc123def456789012345678901234567890abcd").unwrap(),
                subdirectory: Default::default(),
                reference: GitReference::DefaultBranch,
            },
        });

        let canonical1 = CanonicalSpec::from(&spec1);
        let canonical2 = CanonicalSpec::from(&spec2);

        assert_eq!(canonical1, canonical2);
    }

    #[test]
    fn test_git_different_subdirectory() {
        let spec1 = PinnedSourceSpec::Git(PinnedGitSpec {
            git: Url::parse("https://github.com/user/repo").unwrap(),
            source: PinnedGitCheckout {
                commit: GitSha::from_str("abc123def456789012345678901234567890abcd").unwrap(),
                subdirectory: Subdirectory::try_from("subdir1").unwrap(),
                reference: GitReference::DefaultBranch,
            },
        });

        let spec2 = PinnedSourceSpec::Git(PinnedGitSpec {
            git: Url::parse("https://github.com/user/repo").unwrap(),
            source: PinnedGitCheckout {
                commit: GitSha::from_str("abc123def456789012345678901234567890abcd").unwrap(),
                subdirectory: Subdirectory::try_from("subdir2").unwrap(),
                reference: GitReference::DefaultBranch,
            },
        });

        let canonical1 = CanonicalSpec::from(&spec1);
        let canonical2 = CanonicalSpec::from(&spec2);

        assert_ne!(canonical1, canonical2);
    }
}
