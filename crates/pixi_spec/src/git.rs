use std::fmt::Display;

use url::Url;

/// A specification of a package from a git repository.
#[derive(Debug, Clone, Hash, Eq, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct GitSpec {
    /// The git url of the package
    pub git: Url,

    /// The git revision of the package
    #[serde(skip_serializing_if = "Option::is_none", flatten)]
    pub rev: Option<Reference>,

    /// The git subdirectory of the package
    #[serde(skip_serializing_if = "Option::is_none", flatten)]
    pub subdirectory: Option<String>,
}

/// A reference to a specific commit in a git repository.
#[derive(Debug, Clone, Hash, Eq, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reference {
    /// The HEAD commit of a branch.
    Branch(String),

    /// A specific tag.
    Tag(String),

    /// A specific commit.
    Rev(String),
}

impl Display for Reference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reference::Branch(branch) => write!(f, "{}", branch),
            Reference::Tag(tag) => write!(f, "{}", tag),
            Reference::Rev(rev) => write!(f, "{}", rev),
        }
    }
}
