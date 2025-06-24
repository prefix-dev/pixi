use std::io::ErrorKind;
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

/// A utility function to move a file from one location to another by renaming
/// the file if possible and otherwise copying the file and removing the
/// original.
pub(crate) fn move_file(from: &Path, to: &Path) -> Result<(), MoveError> {
    match fs_err::rename(from, to) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == ErrorKind::CrossesDevices => {
            fs_err::copy(from, to).map_err(MoveError::CopyFailed)?;
            fs_err::remove_file(from).map_err(MoveError::FailedToRemove)?;
            Ok(())
        }
        Err(e) => Err(MoveError::MoveFailed(e)),
    }
}
