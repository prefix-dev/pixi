use std::path::Path;

use rattler_conda_types::{package::ArchiveIdentifier, NamelessMatchSpec};
use typed_path::{Utf8NativePathBuf, Utf8TypedPathBuf};

use crate::SpecConversionError;

/// A specification of a package from a git repository.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct PathSpec {
    /// The path to the package
    pub path: Utf8TypedPathBuf,
}

impl PathSpec {
    /// Converts this instance into a [`NamelessMatchSpec`] if the path points
    /// to binary archive.
    pub fn try_into_nameless_match_spec(
        self,
        root_dir: &Path,
    ) -> Result<Option<NamelessMatchSpec>, SpecConversionError> {
        if !self.is_binary() {
            // Not a binary package
            return Ok(None);
        }

        // Convert the path to an absolute path based on the root_dir
        let path = if self.path.is_absolute() {
            self.path
        } else if let Ok(user_path) = self.path.strip_prefix("~/") {
            let home_dir = dirs::home_dir()
                .ok_or_else(|| SpecConversionError::InvalidPath(self.path.to_string()))?;
            let Some(home_dir_str) = home_dir.to_str() else {
                return Err(SpecConversionError::NotUtf8RootDir(home_dir));
            };
            Utf8TypedPathBuf::from(home_dir_str)
                .join(user_path)
                .normalize()
        } else {
            let Some(root_dir_str) = root_dir.to_str() else {
                return Err(SpecConversionError::NotUtf8RootDir(root_dir.to_path_buf()));
            };
            let native_root_dir = Utf8NativePathBuf::from(root_dir_str);
            if !native_root_dir.is_absolute() {
                return Err(SpecConversionError::NonAbsoluteRootDir(
                    root_dir.to_path_buf(),
                ));
            }

            native_root_dir.to_typed_path().join(self.path).normalize()
        };

        // Convert the absolute url to a file:// url
        let local_file_url =
            file_url::file_path_to_url(path.to_path()).expect("failed to convert path to file url");

        Ok(Some(NamelessMatchSpec {
            url: Some(local_file_url),
            ..NamelessMatchSpec::default()
        }))
    }

    /// Converts this instance into a [`PathSourceSpec`] if the path points to a
    /// source package. Otherwise, returns this instance unmodified.
    #[allow(clippy::result_large_err)]
    pub fn try_into_source_path(self) -> Result<PathSourceSpec, Self> {
        if self.is_binary() {
            Err(self)
        } else {
            Ok(PathSourceSpec { path: self.path })
        }
    }

    /// Returns true if this path points to a binary archive.
    pub fn is_binary(&self) -> bool {
        self.path
            .file_name()
            .and_then(ArchiveIdentifier::try_from_path)
            .is_some()
    }
}

/// Path to a source package. Different from [`PathSpec`] in that this type only
/// refers to source packages.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct PathSourceSpec {
    /// The path to the package. Either a directory or an archive.
    pub path: Utf8TypedPathBuf,
}

impl From<PathSourceSpec> for PathSpec {
    fn from(value: PathSourceSpec) -> Self {
        Self { path: value.path }
    }
}
