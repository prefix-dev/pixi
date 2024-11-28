use std::path::Path;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MoveError {
    #[error(transparent)]
    CopyFailed(std::io::Error),

    #[error(transparent)]
    FailedToRemove(std::io::Error),

    #[error(transparent)]
    MoveFailed(std::io::Error),
}

#[cfg(unix)]
const EXDEV: i32 = 18;

#[cfg(windows)]
const EXDEV: i32 = 17;

/// A utility function to move a file from one location to another by renaming
/// the file if possible and otherwise copying the file and removing the
/// original.
pub(crate) fn move_file(from: &Path, to: &Path) -> Result<(), MoveError> {
    if let Err(e) = std::fs::rename(from, to) {
        if e.raw_os_error() == Some(EXDEV) {
            std::fs::copy(from, to).map_err(MoveError::CopyFailed)?;
            std::fs::remove_file(from).map_err(MoveError::FailedToRemove)?
        } else {
            return Err(MoveError::MoveFailed(e));
        }
    }

    Ok(())
}
