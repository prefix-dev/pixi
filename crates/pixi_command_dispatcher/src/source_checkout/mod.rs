use pixi_git::GitError;
use pixi_record::PinnedSourceSpec;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Location of the source code for a package. This will be used as the input
/// for the build process. Archives are unpacked, git clones are checked out,
/// etc.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceCheckout {
    /// The path to where the source is located locally on disk.
    pub path: PathBuf,

    /// The exact source specification
    pub pinned: PinnedSourceSpec,
}

impl SourceCheckout {
    pub fn new(path: impl AsRef<Path>, pinned: PinnedSourceSpec) -> Self {
        Self {
            path: path.as_ref().to_owned(),
            pinned,
        }
    }
}

#[derive(Debug, Error)]
pub enum SourceCheckoutError {
    #[error(transparent)]
    InvalidPath(#[from] InvalidPathError),

    #[error(transparent)]
    GitError(#[from] GitError),
}

#[derive(Debug, Error)]
pub enum InvalidPathError {
    #[error("the path escapes the root directory: {0}")]
    RelativePathEscapesRoot(PathBuf),

    #[error("could not determine the current home directory while resolving: {0}")]
    CouldNotDetermineHomeDirectory(PathBuf),
}
