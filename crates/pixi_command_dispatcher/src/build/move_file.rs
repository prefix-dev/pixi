use std::{io::ErrorKind, path::Path, sync::Arc};

use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum MoveError {
    #[error(transparent)]
    CopyFailed(Arc<std::io::Error>),

    #[error(transparent)]
    FailedToRemove(Arc<std::io::Error>),

    #[error(transparent)]
    MoveFailed(Arc<std::io::Error>),
}

/// A utility function to move a file from one location to another by renaming
/// the file if possible and otherwise copying the file and removing the
/// original.
pub(crate) fn move_file(from: &Path, to: &Path) -> Result<(), MoveError> {
    match fs_err::rename(from, to) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == ErrorKind::CrossesDevices => {
            fs_err::copy(from, to).map_err(|e| MoveError::CopyFailed(Arc::new(e)))?;
            fs_err::remove_file(from).map_err(|e| MoveError::FailedToRemove(Arc::new(e)))?;
            Ok(())
        }
        Err(e) => Err(MoveError::MoveFailed(Arc::new(e))),
    }
}
