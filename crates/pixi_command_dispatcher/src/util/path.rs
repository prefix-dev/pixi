use crate::InvalidPathError;
use pixi_compute_engine::ComputeCtx;
use pixi_path::{AbsPathBuf, AbsPresumedDirPath, AbsPresumedDirPathBuf};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use typed_path::Utf8TypedPath;

/// The root directory of the computations. This is the directory that
/// contains the workspace manifest.
pub struct RootDir(pub AbsPresumedDirPathBuf);

impl Deref for RootDir {
    type Target = AbsPresumedDirPath;

    fn deref(&self) -> &Self::Target {
        self.0.as_path()
    }
}

pub(crate) trait RootDirExt {
    /// Returns the root directory of the computations. IF a relative path is
    /// provided, it is resolved against this directory (unless another base
    /// directory is specified).
    fn root_dir(&self) -> &RootDir;

    /// Resolves the source path to a full path.
    ///
    /// This function does not check if the path exists and also does not follow
    /// symlinks.
    fn resolve_typed_path(&self, path_spec: Utf8TypedPath) -> Result<AbsPathBuf, InvalidPathError> {
        if path_spec.is_absolute() {
            // SAFETY: we checked that the path is absolute
            Ok(unsafe { AbsPathBuf::new_unchecked(PathBuf::from(path_spec.as_str())) })
        } else if let Ok(user_path) = path_spec.strip_prefix("~/") {
            let home_dir = dirs::home_dir().ok_or_else(|| {
                InvalidPathError::CouldNotDetermineHomeDirectory(PathBuf::from(path_spec.as_str()))
            })?;
            let home_dir = AbsPathBuf::new(home_dir)
                .expect("the home directory is absolute")
                .into_assume_dir();
            home_dir
                .join(Path::new(user_path.as_str()))
                .normalized()
                .map_err(Into::into)
        } else {
            let native_path = Path::new(path_spec.as_str());
            self.root_dir()
                .join(native_path)
                .normalized()
                .map_err(Into::into)
        }
    }
}

impl RootDirExt for ComputeCtx {
    fn root_dir(&self) -> &RootDir {
        self.global_data().get()
    }
}
