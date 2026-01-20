use crate::command_dispatcher::url::UrlError;
use miette::Diagnostic;
use pixi_git::GitError;
use pixi_path::{AbsPathBuf, NormalizeError};
use pixi_record::PinnedSourceSpec;
use std::path::PathBuf;
use thiserror::Error;

/// Location of the source code for a package. This will be used as the input
/// for the build process. Archives are unpacked, git clones are checked out,
/// etc.
#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SourceCheckout {
    /// The path to where the source is located locally on disk.
    pub path: AbsPathBuf,

    /// The exact source specification
    pub pinned: PinnedSourceSpec,
}

impl SourceCheckout {
    /// Returns true if the contents of the source checkout are immutable.
    pub fn is_immutable(&self) -> bool {
        self.pinned.is_immutable()
    }
}

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
