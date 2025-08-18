use std::fmt::Display;
use std::path::{Path, PathBuf};

use itertools::Either;
use rattler_conda_types::{NamelessMatchSpec, package::ArchiveIdentifier};
use serde_with::serde_as;
use typed_path::{Utf8NativePathBuf, Utf8TypedPathBuf};

use crate::{BinarySpec, SpecConversionError};

/// A specification of a package from a path.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct PathSpec {
    /// The path to the package
    pub path: Utf8TypedPathBuf,
}

impl PathSpec {
    /// Constructs a new [`PathSpec`] from the given path.
    pub fn new(path: impl Into<Utf8TypedPathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Converts this instance into a [`NamelessMatchSpec`] if the path points
    /// to binary archive.
    pub fn try_into_nameless_match_spec(
        self,
        root_dir: &Path,
    ) -> Result<Option<NamelessMatchSpec>, SpecConversionError> {
        match self.into_source_or_binary() {
            Either::Left(_source) => Ok(None),
            Either::Right(binary) => Ok(Some(binary.try_into_nameless_match_spec(root_dir)?)),
        }
    }

    /// Resolves the path relative to `root_dir`. If the path is absolute,
    /// it is returned verbatim.
    ///
    /// May return an error if the path is prefixed with `~` and the home
    /// directory is undefined.
    pub fn resolve(&self, root_dir: impl AsRef<Path>) -> Result<PathBuf, SpecConversionError> {
        resolve_path(Path::new(self.path.as_str()), root_dir)
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

    /// Converts this instance into a [`PathSourceSpec`] if the path points to a
    /// source package. Or to a [`NamelessMatchSpec`] otherwise.
    pub fn into_source_or_binary(self) -> Either<PathSourceSpec, PathBinarySpec> {
        if self.is_binary() {
            Either::Right(PathBinarySpec { path: self.path })
        } else {
            Either::Left(PathSourceSpec { path: self.path })
        }
    }
}

impl Display for PathSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.path)
    }
}

// TODO: Contribute `impl FromStr for Utf8TypedPathBuf` to typed-path
// to continue using `serde_as` and remove manual implementations of
// serialization and deserialization below. See git blame history
// right before this line was added.

/// Path to a source package. Different from [`PathSpec`] in that this type only
/// refers to source packages.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct PathSourceSpec {
    /// The path to the package. Either a directory or an archive.
    pub path: Utf8TypedPathBuf,
}

impl Display for PathSourceSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.path)
    }
}

impl serde::Serialize for PathSourceSpec {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(serde::Serialize)]
        struct Raw {
            path: String,
        }

        Raw {
            path: self.path.to_string(),
        }
        .serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for PathSourceSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Raw {
            path: String,
        }

        Raw::deserialize(deserializer).map(|raw| PathSourceSpec {
            path: raw.path.into(),
        })
    }
}

impl From<PathSourceSpec> for PathSpec {
    fn from(value: PathSourceSpec) -> Self {
        Self { path: value.path }
    }
}

impl PathSourceSpec {
    /// Resolves the path relative to `root_dir`. If the path is absolute,
    /// it is returned verbatim.
    ///
    /// May return an error if the path is prefixed with `~` and the home
    /// directory is undefined.
    pub fn resolve(&self, root_dir: impl AsRef<Path>) -> Result<PathBuf, SpecConversionError> {
        resolve_path(Path::new(self.path.as_str()), root_dir)
    }
}

/// Path to a source package. Different from [`PathSpec`] in that this type only
/// refers to source packages.
#[serde_as]
#[derive(Debug, Clone, Hash, Eq, PartialEq, ::serde::Serialize)]
pub struct PathBinarySpec {
    /// The path to the package. Either a directory or an archive.
    #[serde_as(as = "serde_with::DisplayFromStr")]
    pub path: Utf8TypedPathBuf,
}

impl From<PathBinarySpec> for PathSpec {
    fn from(value: PathBinarySpec) -> Self {
        Self { path: value.path }
    }
}

impl From<PathBinarySpec> for BinarySpec {
    fn from(value: PathBinarySpec) -> Self {
        Self::Path(value)
    }
}

impl PathBinarySpec {
    /// Resolves the path relative to `root_dir`. If the path is absolute,
    /// it is returned verbatim.
    ///
    /// May return an error if the path is prefixed with `~` and the home
    /// directory is undefined.
    pub fn resolve(&self, root_dir: impl AsRef<Path>) -> Result<PathBuf, SpecConversionError> {
        resolve_path(Path::new(self.path.as_str()), root_dir)
    }

    /// Converts this instance into a [`NamelessMatchSpec`]
    pub fn try_into_nameless_match_spec(
        self,
        root_dir: &Path,
    ) -> Result<NamelessMatchSpec, SpecConversionError> {
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

        Ok(NamelessMatchSpec {
            url: Some(local_file_url),
            ..NamelessMatchSpec::default()
        })
    }
}

/// Resolves the path relative to `root_dir`. If the path is absolute,
/// it is returned verbatim.
///
/// May return an error if the path is prefixed with `~` and the home
/// directory is undefined.
fn resolve_path(path: &Path, root_dir: impl AsRef<Path>) -> Result<PathBuf, SpecConversionError> {
    if path.is_absolute() {
        Ok(PathBuf::from(path))
    } else if let Ok(user_path) = path.strip_prefix("~/") {
        let home_dir = dirs::home_dir()
            .ok_or_else(|| SpecConversionError::InvalidPath(path.display().to_string()))?;
        Ok(home_dir.join(user_path))
    } else {
        Ok(root_dir.as_ref().join(path))
    }
}
