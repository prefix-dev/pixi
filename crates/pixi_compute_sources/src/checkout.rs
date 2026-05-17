//! [`SourceCheckout`] (the shared checkout result), and the
//! [`SourceCheckoutError`] / [`InvalidPathError`] families.

use std::path::PathBuf;

use miette::Diagnostic;
use pixi_git::{GitError, source::Fetch as GitFetch};
use pixi_path::{AbsPathBuf, NormalizeError};
use pixi_record::{PinnedGitSpec, PinnedSourceSpec};
use pixi_url::UrlError;
use thiserror::Error;

/// Location of the source code for a package on disk. Produced by the
/// per-spec checkout Keys; downstream Keys read from `path` and rely
/// on `pinned` for cache identity.
#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SourceCheckout {
    /// Local path to the source.
    pub path: AbsPathBuf,

    /// The exact source specification.
    pub pinned: PinnedSourceSpec,
}

impl SourceCheckout {
    /// True if the contents of the source checkout are immutable.
    pub fn is_immutable(&self) -> bool {
        self.pinned.is_immutable()
    }

    /// Construct a [`SourceCheckout`] from a completed git fetch and
    /// its pinned spec, applying the `pinned.source.subdirectory`.
    pub(crate) fn from_git(fetch: GitFetch, pinned: PinnedGitSpec) -> Self {
        let root_dir = AbsPathBuf::new(fetch.into_path())
            .expect("git checkout returned a relative path")
            .into_assume_dir();

        let path = if !pinned.source.subdirectory.is_empty() {
            root_dir
                .join(pinned.source.subdirectory.as_path())
                .into_assume_dir()
        } else {
            root_dir
        };

        Self {
            path: path.into(),
            pinned: PinnedSourceSpec::Git(pinned),
        }
    }
}

/// Failure modes for source checkouts.
#[derive(Debug, Clone, Error, Diagnostic)]
pub enum SourceCheckoutError {
    #[error(transparent)]
    InvalidPath(#[from] InvalidPathError),

    #[error(transparent)]
    GitError(#[from] GitError),

    #[error(transparent)]
    UrlError(#[from] UrlError),

    #[error("the manifest path {0} should have a parent directory")]
    ParentDir(PathBuf),

    #[error("the subdirectory {0} does not exist in the source checkout")]
    SubdirDoesNotExist(PathBuf),

    #[error("the subdirectory {0} refers to a file in the source checkout")]
    SubdirIsAFile(PathBuf),
}

/// Path-level failures encountered while resolving a relative source
/// spec against a root directory.
#[derive(Debug, Clone, Error)]
pub enum InvalidPathError {
    #[error("the path escapes the root directory: {0}")]
    RelativePathEscapesRoot(PathBuf),

    #[error("could not determine the current home directory while resolving: {0}")]
    CouldNotDetermineHomeDirectory(PathBuf),
}

impl From<NormalizeError> for InvalidPathError {
    fn from(value: NormalizeError) -> Self {
        match value {
            NormalizeError::EscapesRoot(path) => InvalidPathError::RelativePathEscapesRoot(path),
        }
    }
}
