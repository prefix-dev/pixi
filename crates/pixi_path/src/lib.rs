//! Type-safe path wrappers with compile-time guarantees.
//!
//! This crate provides path types that guarantee certain properties at the type level:
//!
//! - [`AbsPath`] / [`AbsPathBuf`]: A path that is guaranteed to be absolute
//! - [`AbsPresumedDirPath`] / [`AbsPresumedDirPathBuf`]: An absolute path that is *presumed*
//!   to be a directory (no filesystem check)
//! - [`AbsPresumedFilePath`] / [`AbsPresumedFilePathBuf`]: An absolute path that is *presumed*
//!   to be a file (no filesystem check)
//!
//! # No Implicit Filesystem Checks
//!
//! This library does **not** perform filesystem checks implicitly. The "Presumed" types
//! represent *intent* - they indicate how you expect to use the path, not what actually
//! exists on the filesystem. Use [`AbsPath::assume_dir()`] or [`AbsPathBuf::into_assume_dir()`]
//! to convert an absolute path to a presumed directory path.
//!
//! # Naming Convention
//!
//! Types follow the same naming convention as the standard library:
//! - Types ending in `Path` are borrowed (like [`std::path::Path`])
//! - Types ending in `PathBuf` are owned (like [`std::path::PathBuf`])
//!
//! # Example
//!
//! ```
//! use pixi_path::{AbsPath, AbsPathBuf};
//! use std::path::Path;
//!
//! // Create an absolute path (will fail for relative paths)
//! # #[cfg(unix)]
//! let abs_path = AbsPath::new(Path::new("/usr/bin")).unwrap();
//! # #[cfg(windows)]
//! # let abs_path = AbsPath::new(Path::new("C:\\Windows")).unwrap();
//!
//! // Treat it as a directory path (no filesystem check)
//! let dir_path = abs_path.assume_dir();
//!
//! // Create an owned absolute path
//! # #[cfg(unix)]
//! let abs_buf = AbsPathBuf::new("/usr/bin").unwrap();
//! # #[cfg(windows)]
//! # let abs_buf = AbsPathBuf::new("C:\\Windows").unwrap();
//!
//! // Convert to a directory path (no filesystem check)
//! let dir_buf = abs_buf.into_assume_dir();
//! ```

use std::borrow::Borrow;
use std::ops::Deref;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

pub mod normalize;

/// Error type for path validation failures.
#[derive(Debug, Error)]
pub enum PathError {
    /// The path is not absolute.
    #[error("{0} is not an absolute path")]
    NotAbsolute(PathBuf),
}

/// Error type for path normalization failures.
#[derive(Debug, Error)]
pub enum NormalizeError {
    /// The path escapes the root directory (e.g., too many `..` components).
    #[error("the path escapes the root directory: {0}")]
    EscapesRoot(PathBuf),
}

/// A borrowed reference to an absolute path.
///
/// This is the borrowed equivalent of [`AbsPathBuf`], similar to how
/// [`Path`] is the borrowed equivalent of [`PathBuf`].
///
/// # Invariants
///
/// An `AbsPath` is always absolute. This is enforced at construction time.
#[derive(Hash, Eq, PartialEq, Debug)]
#[repr(transparent)]
pub struct AbsPath(Path);

impl AbsPath {
    /// Creates a new `AbsPath` from a `Path` reference.
    ///
    /// # Errors
    ///
    /// Returns [`PathError::NotAbsolute`] if the path is not absolute.
    ///
    /// # Example
    ///
    /// ```
    /// use pixi_path::AbsPath;
    /// use std::path::Path;
    ///
    /// # #[cfg(unix)]
    /// assert!(AbsPath::new(Path::new("/usr/bin")).is_ok());
    /// # #[cfg(unix)]
    /// assert!(AbsPath::new(Path::new("relative/path")).is_err());
    /// ```
    pub fn new(path: &Path) -> Result<&Self, PathError> {
        if !path.is_absolute() {
            Err(PathError::NotAbsolute(path.to_path_buf()))
        } else {
            // SAFETY: AbsPath is #[repr(transparent)] over Path
            Ok(unsafe { &*(path as *const Path as *const AbsPath) })
        }
    }

    /// Creates a new `AbsPath` without checking if the path is absolute.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the path is absolute.
    #[inline]
    pub unsafe fn new_unchecked(path: &Path) -> &Self {
        debug_assert!(path.is_absolute(), "path must be absolute");
        // SAFETY: AbsPath is #[repr(transparent)] over Path
        unsafe { &*(path as *const Path as *const AbsPath) }
    }

    /// Returns the underlying standard library [`Path`] reference.
    #[inline]
    pub fn as_std_path(&self) -> &Path {
        &self.0
    }

    /// Converts this to a standard library [`PathBuf`].
    #[inline]
    pub fn to_std_path_buf(&self) -> PathBuf {
        self.0.to_path_buf()
    }

    /// Converts this borrowed reference to an owned [`AbsPathBuf`].
    #[inline]
    pub fn to_path_buf(&self) -> AbsPathBuf {
        AbsPathBuf(self.0.to_path_buf())
    }

    /// Returns the directory of this path.
    ///
    /// - If this path is a directory, returns itself.
    /// - If this path is a file, returns the parent directory.
    /// - Returns `None` if the path doesn't exist, is something else (e.g., a symlink
    ///   to something that doesn't exist), or if a file has no parent.
    pub fn directory(&self) -> Option<&AbsPresumedDirPath> {
        match self.0.metadata() {
            Ok(meta) if meta.is_dir() => {
                // SAFETY: We know self is absolute, and it's a directory
                Some(unsafe { AbsPresumedDirPath::new_unchecked(&self.0) })
            }
            Ok(meta) if meta.is_file() => {
                // SAFETY: Parent of an absolute path is always absolute
                self.0
                    .parent()
                    .map(|p| unsafe { AbsPresumedDirPath::new_unchecked(p) })
            }
            _ => None,
        }
    }

    /// Returns the parent directory of this path.
    ///
    /// Returns `None` if the path has no parent (e.g., it's a root path like `/` or `C:\`).
    pub fn parent(&self) -> Option<&AbsPresumedDirPath> {
        self.0
            .parent()
            .map(|p| unsafe { AbsPresumedDirPath::new_unchecked(p) })
    }

    /// Returns a normalized version of this path, resolving `.` and `..` components.
    ///
    /// This does not access the filesystem and works purely on the path components.
    ///
    /// # Errors
    ///
    /// Returns [`NormalizeError::EscapesRoot`] if the path contains too many `..` components
    /// that would escape the root directory.
    ///
    /// # Example
    ///
    /// ```
    /// use pixi_path::AbsPath;
    /// use std::path::Path;
    ///
    /// # #[cfg(unix)]
    /// let path = AbsPath::new(Path::new("/usr/bin/../lib")).unwrap();
    /// # #[cfg(unix)]
    /// assert_eq!(path.normalized().unwrap().as_std_path(), Path::new("/usr/lib"));
    /// ```
    pub fn normalized(&self) -> Result<AbsPathBuf, NormalizeError> {
        let mut components = self.0.components().peekable();
        let mut ret = if let Some(c @ Component::Prefix(..)) = components.peek().copied() {
            components.next();
            PathBuf::from(c.as_os_str())
        } else {
            PathBuf::new()
        };

        for component in components {
            match component {
                Component::Prefix(..) => unreachable!(),
                Component::RootDir => {
                    ret.push(component.as_os_str());
                }
                Component::CurDir => {}
                Component::ParentDir => {
                    if !ret.pop() {
                        return Err(NormalizeError::EscapesRoot(self.0.to_path_buf()));
                    }
                }
                Component::Normal(c) => {
                    ret.push(c);
                }
            }
        }

        // SAFETY: We started with an absolute path and only manipulated components,
        // maintaining the absolute prefix/root.
        Ok(unsafe { AbsPathBuf::new_unchecked(ret) })
    }

    /// Joins this path with a relative path component.
    ///
    /// The resulting path is always absolute since the base is absolute.
    #[inline]
    pub fn join(&self, path: impl AsRef<Path>) -> AbsPathBuf {
        AbsPathBuf(self.0.join(path))
    }

    /// Creates the directory at this path, including all parent directories.
    ///
    /// This is equivalent to [`fs_err::create_dir_all`].
    ///
    /// # Errors
    ///
    /// Returns an [`std::io::Error`] if the directory could not be created.
    pub fn create_dir_all(&self) -> std::io::Result<&AbsPresumedDirPath> {
        fs_err::create_dir_all(&self.0)?;
        // SAFETY: We just created the directory, so it exists and is a directory
        Ok(unsafe { AbsPresumedDirPath::new_unchecked(&self.0) })
    }

    /// Treats this path as a directory path.
    ///
    /// This is about intent - no filesystem check is performed. The returned type
    /// indicates that this path is *presumed* to be a directory.
    #[inline]
    pub fn assume_dir(&self) -> &AbsPresumedDirPath {
        // SAFETY: AbsPresumedDirPath is #[repr(transparent)] over AbsPath
        unsafe { &*(&self.0 as *const Path as *const AbsPresumedDirPath) }
    }

    /// Treats this path as a file path.
    ///
    /// This is about intent - no filesystem check is performed. The returned type
    /// indicates that this path is *presumed* to be a file.
    #[inline]
    pub fn assume_file(&self) -> &AbsPresumedFilePath {
        // SAFETY: AbsPresumedFilePath is #[repr(transparent)] over AbsPath
        unsafe { &*(&self.0 as *const Path as *const AbsPresumedFilePath) }
    }

    /// Returns this path as a file path if it exists and is a file.
    ///
    /// Unlike [`assume_file`](Self::assume_file), this method performs a filesystem check.
    /// Returns `Some` if the path exists and is a regular file, `None` otherwise.
    #[inline]
    pub fn as_file(&self) -> Option<&AbsPresumedFilePath> {
        if self.0.is_file() {
            Some(self.assume_file())
        } else {
            None
        }
    }

    /// Returns this path as a directory path if it exists and is a directory.
    ///
    /// Unlike [`assume_dir`](Self::assume_dir), this method performs a filesystem check.
    /// Returns `Some` if the path exists and is a directory, `None` otherwise.
    #[inline]
    pub fn as_dir(&self) -> Option<&AbsPresumedDirPath> {
        if self.0.is_dir() {
            Some(self.assume_dir())
        } else {
            None
        }
    }

    /// Returns this path as a directory, or if it's a file, returns its parent directory.
    ///
    /// This performs a filesystem check to determine if the path is a file.
    /// If the path is a file, returns its parent directory.
    /// Otherwise, returns the path itself as a directory.
    #[inline]
    pub fn as_dir_or_file_parent(&self) -> &AbsPresumedDirPath {
        if let Some(file) = self.as_file() {
            file.parent()
        } else {
            self.assume_dir()
        }
    }

    /// Returns `true` if the path points at an existing entity.
    ///
    /// This is equivalent to [`Path::exists`].
    #[inline]
    pub fn exists(&self) -> bool {
        self.0.exists()
    }

    /// Returns `true` if the path exists on disk and is pointing at a regular file.
    ///
    /// This is equivalent to [`Path::is_file`].
    #[inline]
    pub fn is_file(&self) -> bool {
        self.0.is_file()
    }

    /// Returns `true` if the path exists on disk and is pointing at a directory.
    ///
    /// This is equivalent to [`Path::is_dir`].
    #[inline]
    pub fn is_dir(&self) -> bool {
        self.0.is_dir()
    }

    /// Returns `true` if the path exists on disk and is pointing at a symbolic link.
    ///
    /// This is equivalent to [`Path::is_symlink`].
    #[inline]
    pub fn is_symlink(&self) -> bool {
        self.0.is_symlink()
    }

    /// Queries the file system to get information about a file, directory, etc.
    ///
    /// This is equivalent to [`Path::metadata`].
    #[inline]
    pub fn metadata(&self) -> std::io::Result<std::fs::Metadata> {
        self.0.metadata()
    }

    /// Returns an object that implements [`Display`] for safely printing paths.
    ///
    /// This is equivalent to [`Path::display`].
    ///
    /// [`Display`]: std::fmt::Display
    #[inline]
    pub fn display(&self) -> std::path::Display<'_> {
        self.0.display()
    }

    /// Returns a path that, when joined onto `base`, yields `self`.
    ///
    /// # Errors
    ///
    /// If `base` is not a prefix of `self`, returns an error.
    ///
    /// # Example
    ///
    /// ```
    /// use pixi_path::AbsPath;
    /// use std::path::Path;
    ///
    /// # #[cfg(unix)]
    /// let path = AbsPath::new(Path::new("/usr/lib/foo.so")).unwrap();
    /// # #[cfg(unix)]
    /// let base = AbsPath::new(Path::new("/usr")).unwrap();
    /// # #[cfg(unix)]
    /// assert_eq!(path.strip_prefix(base).unwrap(), Path::new("lib/foo.so"));
    /// ```
    #[inline]
    pub fn strip_prefix(&self, base: &AbsPath) -> Result<&Path, std::path::StripPrefixError> {
        self.0.strip_prefix(&base.0)
    }
}

impl AsRef<Path> for AbsPath {
    #[inline]
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl ToOwned for AbsPath {
    type Owned = AbsPathBuf;

    fn to_owned(&self) -> Self::Owned {
        self.to_path_buf()
    }
}

/// An owned absolute path.
///
/// This is the owned equivalent of [`AbsPath`], similar to how
/// [`PathBuf`] is the owned equivalent of [`Path`].
///
/// # Invariants
///
/// An `AbsPathBuf` always contains an absolute path. This is enforced at
/// construction time.
#[derive(Clone, Hash, Eq, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct AbsPathBuf(PathBuf);

impl AbsPathBuf {
    /// Creates a new `AbsPathBuf` from a path.
    ///
    /// # Errors
    ///
    /// Returns [`PathError::NotAbsolute`] if the path is not absolute.
    ///
    /// # Example
    ///
    /// ```
    /// use pixi_path::AbsPathBuf;
    ///
    /// # #[cfg(unix)]
    /// assert!(AbsPathBuf::new("/usr/bin").is_ok());
    /// # #[cfg(unix)]
    /// assert!(AbsPathBuf::new("relative/path").is_err());
    /// ```
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, PathError> {
        let path = path.into();
        if !path.is_absolute() {
            Err(PathError::NotAbsolute(path))
        } else {
            Ok(Self(path))
        }
    }

    /// Creates a new `AbsPathBuf` without checking if the path is absolute.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the path is absolute.
    #[inline]
    pub unsafe fn new_unchecked(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        debug_assert!(path.is_absolute(), "path must be absolute");
        Self(path)
    }

    /// Returns the underlying standard library [`Path`] reference.
    #[inline]
    pub fn as_std_path(&self) -> &Path {
        self.0.as_path()
    }

    /// Returns a borrowed [`AbsPath`].
    #[inline]
    pub fn as_path(&self) -> &AbsPath {
        // SAFETY: We maintain the invariant that self.0 is always absolute
        unsafe { AbsPath::new_unchecked(self.0.as_path()) }
    }

    /// Consumes this and returns the inner [`PathBuf`].
    #[inline]
    pub fn into_std_path_buf(self) -> PathBuf {
        self.0
    }

    /// Creates the directory at this path, including all parent directories,
    /// and returns the path as an [`AbsPresumedDirPathBuf`].
    ///
    /// This is equivalent to [`fs_err::create_dir_all`].
    ///
    /// # Errors
    ///
    /// Returns an [`std::io::Error`] if the directory could not be created.
    pub fn into_create_dir_all(self) -> std::io::Result<AbsPresumedDirPathBuf> {
        fs_err::create_dir_all(&self.0)?;
        // SAFETY: We just created the directory, so it exists and is a directory
        Ok(unsafe { AbsPresumedDirPathBuf::new_unchecked(self.0) })
    }

    /// Converts this path to a directory path.
    ///
    /// This is about intent - no filesystem check is performed. The returned type
    /// indicates that this path is *presumed* to be a directory.
    #[inline]
    pub fn into_assume_dir(self) -> AbsPresumedDirPathBuf {
        AbsPresumedDirPathBuf(self)
    }

    /// Converts this path to a file path.
    ///
    /// This is about intent - no filesystem check is performed. The returned type
    /// indicates that this path is *presumed* to be a file.
    #[inline]
    pub fn into_assume_file(self) -> AbsPresumedFilePathBuf {
        AbsPresumedFilePathBuf(self)
    }

    /// Returns a normalized version of this path, removing `.` and `..` components.
    ///
    /// # Errors
    ///
    /// Returns [`NormalizeError::EscapesRoot`] if the path contains too many `..` components
    /// that would escape the root directory.
    pub fn normalized(&self) -> Result<AbsPathBuf, NormalizeError> {
        self.as_path().normalized()
    }
}

impl Deref for AbsPathBuf {
    type Target = AbsPath;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_path()
    }
}

impl AsRef<Path> for AbsPathBuf {
    #[inline]
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<AbsPath> for AbsPathBuf {
    #[inline]
    fn as_ref(&self) -> &AbsPath {
        self.as_path()
    }
}

impl Borrow<AbsPath> for AbsPathBuf {
    #[inline]
    fn borrow(&self) -> &AbsPath {
        self.as_path()
    }
}

impl From<AbsPathBuf> for PathBuf {
    #[inline]
    fn from(path: AbsPathBuf) -> Self {
        path.0
    }
}

impl std::fmt::Display for AbsPathBuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.display())
    }
}

/// A borrowed reference to an absolute path that is *presumed* to be a directory.
///
/// This is the borrowed equivalent of [`AbsPresumedDirPathBuf`].
///
/// # Semantics
///
/// This type represents a path that is *intended* to be a directory. No filesystem
/// check is performed - use [`AbsPath::assume_dir()`] to create one from an [`AbsPath`].
///
/// # Invariants
///
/// An `AbsPresumedDirPath` is always absolute.
#[derive(Hash, Eq, PartialEq, Debug)]
#[repr(transparent)]
pub struct AbsPresumedDirPath(AbsPath);

impl AbsPresumedDirPath {
    /// Creates a new `AbsPresumedDirPath` without checking if the path is
    /// absolute or a directory.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the path is absolute.
    #[inline]
    pub unsafe fn new_unchecked(path: &Path) -> &Self {
        debug_assert!(path.is_absolute(), "path must be absolute");
        // SAFETY: AbsPresumedDirPath has the same layout as Path through
        // AbsPath which is #[repr(transparent)]
        unsafe { &*(path as *const Path as *const AbsPresumedDirPath) }
    }

    /// Returns the underlying standard library [`Path`] reference.
    #[inline]
    pub fn as_std_path(&self) -> &Path {
        self.0.as_std_path()
    }

    /// Returns a borrowed [`AbsPath`].
    #[inline]
    pub fn as_absolute_path(&self) -> &AbsPath {
        &self.0
    }

    /// Converts this to a standard library [`PathBuf`].
    #[inline]
    pub fn to_std_path_buf(&self) -> PathBuf {
        self.0.to_std_path_buf()
    }

    /// Converts this borrowed reference to an owned [`AbsPresumedDirPathBuf`].
    #[inline]
    pub fn to_path_buf(&self) -> AbsPresumedDirPathBuf {
        AbsPresumedDirPathBuf(self.0.to_path_buf())
    }

    /// Returns a normalized version of this directory path, resolving `.` and `..` components.
    ///
    /// This does not access the filesystem and works purely on the path components.
    ///
    /// # Errors
    ///
    /// Returns [`NormalizeError::EscapesRoot`] if the path contains too many `..` components
    /// that would escape the root directory.
    pub fn normalized(&self) -> Result<AbsPresumedDirPathBuf, NormalizeError> {
        let normalized = self.0.normalized()?;
        Ok(AbsPresumedDirPathBuf(normalized))
    }
}

impl Deref for AbsPresumedDirPath {
    type Target = AbsPath;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<Path> for AbsPresumedDirPath {
    #[inline]
    fn as_ref(&self) -> &Path {
        self.0.as_std_path()
    }
}

impl AsRef<AbsPath> for AbsPresumedDirPath {
    #[inline]
    fn as_ref(&self) -> &AbsPath {
        &self.0
    }
}

impl ToOwned for AbsPresumedDirPath {
    type Owned = AbsPresumedDirPathBuf;

    fn to_owned(&self) -> Self::Owned {
        self.to_path_buf()
    }
}

/// An owned absolute path that is *presumed* to be a directory.
///
/// This is the owned equivalent of [`AbsPresumedDirPath`].
///
/// # Semantics
///
/// This type represents a path that is *intended* to be a directory. No filesystem
/// check is performed - use [`AbsPathBuf::into_assume_dir()`] to create one from an [`AbsPathBuf`].
///
/// # Invariants
///
/// An `AbsPresumedDirPathBuf` always contains an absolute path.
#[derive(Clone, Hash, Eq, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct AbsPresumedDirPathBuf(AbsPathBuf);

impl AbsPresumedDirPathBuf {
    /// Creates a new `AbsPresumedDirPathBuf` without checking if the path is
    /// absolute or a directory.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the path is absolute.
    #[inline]
    pub unsafe fn new_unchecked(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        debug_assert!(path.is_absolute(), "path must be absolute");
        // SAFETY: The caller guarantees the path is absolute
        Self(unsafe { AbsPathBuf::new_unchecked(path) })
    }

    /// Returns the underlying standard library [`Path`] reference.
    #[inline]
    pub fn as_std_path(&self) -> &Path {
        self.0.as_std_path()
    }

    /// Returns a borrowed [`AbsPresumedDirPath`].
    #[inline]
    pub fn as_path(&self) -> &AbsPresumedDirPath {
        // SAFETY: We maintain the invariant that self.0 is always an absolute directory
        unsafe { AbsPresumedDirPath::new_unchecked(self.0.as_std_path()) }
    }

    /// Returns a reference to the inner [`AbsPathBuf`].
    #[inline]
    pub fn as_absolute_path_buf(&self) -> &AbsPathBuf {
        &self.0
    }

    /// Consumes this and returns the inner [`AbsPathBuf`].
    #[inline]
    pub fn into_absolute_path_buf(self) -> AbsPathBuf {
        self.0
    }

    /// Consumes this and returns the inner [`PathBuf`].
    #[inline]
    pub fn into_std_path_buf(self) -> PathBuf {
        self.0.into_std_path_buf()
    }

    /// Returns a normalized version of this path, removing `.` and `..` components,
    /// while retaining the presumption that this is a directory path.
    ///
    /// # Errors
    ///
    /// Returns [`NormalizeError::EscapesRoot`] if the path contains too many `..` components
    /// that would escape the root directory.
    pub fn normalized(&self) -> Result<AbsPresumedDirPathBuf, NormalizeError> {
        self.as_path().normalized()
    }
}

impl Deref for AbsPresumedDirPathBuf {
    type Target = AbsPresumedDirPath;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_path()
    }
}

impl AsRef<Path> for AbsPresumedDirPathBuf {
    #[inline]
    fn as_ref(&self) -> &Path {
        self.0.as_std_path()
    }
}

impl AsRef<AbsPath> for AbsPresumedDirPathBuf {
    #[inline]
    fn as_ref(&self) -> &AbsPath {
        self.0.as_path()
    }
}

impl AsRef<AbsPresumedDirPath> for AbsPresumedDirPathBuf {
    #[inline]
    fn as_ref(&self) -> &AbsPresumedDirPath {
        self.as_path()
    }
}

impl Borrow<AbsPresumedDirPath> for AbsPresumedDirPathBuf {
    #[inline]
    fn borrow(&self) -> &AbsPresumedDirPath {
        self.as_path()
    }
}

impl From<AbsPresumedDirPathBuf> for PathBuf {
    #[inline]
    fn from(path: AbsPresumedDirPathBuf) -> Self {
        path.0.into()
    }
}

impl From<AbsPresumedDirPathBuf> for AbsPathBuf {
    #[inline]
    fn from(path: AbsPresumedDirPathBuf) -> Self {
        path.0
    }
}

/// A borrowed reference to an absolute path that is *presumed* to be a file.
///
/// This is the borrowed equivalent of [`AbsPresumedFilePathBuf`].
///
/// # Semantics
///
/// This type represents a path that is *intended* to be a file. No filesystem
/// check is performed - use [`AbsPath::assume_file()`] to create one from an [`AbsPath`].
///
/// # Invariants
///
/// An `AbsPresumedFilePath` is always absolute.
#[derive(Hash, Eq, PartialEq, Debug)]
#[repr(transparent)]
pub struct AbsPresumedFilePath(AbsPath);

impl AbsPresumedFilePath {
    /// Creates a new `AbsPresumedFilePath` without checking if the path is
    /// absolute or a file.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the path is absolute.
    #[inline]
    pub unsafe fn new_unchecked(path: &Path) -> &Self {
        debug_assert!(path.is_absolute(), "path must be absolute");
        // SAFETY: AbsPresumedFilePath has the same layout as Path through
        // AbsPath which is #[repr(transparent)]
        unsafe { &*(path as *const Path as *const AbsPresumedFilePath) }
    }

    /// Returns the underlying standard library [`Path`] reference.
    #[inline]
    pub fn as_std_path(&self) -> &Path {
        self.0.as_std_path()
    }

    /// Returns a borrowed [`AbsPath`].
    #[inline]
    pub fn as_absolute_path(&self) -> &AbsPath {
        &self.0
    }

    /// Returns the parent directory of this file path.
    ///
    /// Since this is a file path, it always has a parent directory.
    #[inline]
    pub fn parent(&self) -> &AbsPresumedDirPath {
        // SAFETY: A file path always has a parent directory.
        // AbsPresumedDirPath is #[repr(transparent)] over AbsPath.
        let parent = self
            .0
            .as_std_path()
            .parent()
            .expect("file path must have a parent");
        unsafe { AbsPresumedDirPath::new_unchecked(parent) }
    }

    /// Converts this to a standard library [`PathBuf`].
    #[inline]
    pub fn to_std_path_buf(&self) -> PathBuf {
        self.0.to_std_path_buf()
    }

    /// Converts this borrowed reference to an owned [`AbsPresumedFilePathBuf`].
    #[inline]
    pub fn to_path_buf(&self) -> AbsPresumedFilePathBuf {
        AbsPresumedFilePathBuf(self.0.to_path_buf())
    }

    /// Returns a normalized version of this path, removing `.` and `..` components,
    /// while retaining the presumption that this is a file path.
    ///
    /// # Errors
    ///
    /// Returns [`NormalizeError::EscapesRoot`] if the path contains too many `..` components
    /// that would escape the root directory.
    pub fn normalized(&self) -> Result<AbsPresumedFilePathBuf, NormalizeError> {
        let normalized = self.0.normalized()?;
        Ok(AbsPresumedFilePathBuf(normalized))
    }
}

impl Deref for AbsPresumedFilePath {
    type Target = AbsPath;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<Path> for AbsPresumedFilePath {
    #[inline]
    fn as_ref(&self) -> &Path {
        self.0.as_std_path()
    }
}

impl AsRef<AbsPath> for AbsPresumedFilePath {
    #[inline]
    fn as_ref(&self) -> &AbsPath {
        &self.0
    }
}

impl ToOwned for AbsPresumedFilePath {
    type Owned = AbsPresumedFilePathBuf;

    fn to_owned(&self) -> Self::Owned {
        self.to_path_buf()
    }
}

/// An owned absolute path that is presumed to be a file.
///
/// This is the owned equivalent of [`AbsPresumedFilePath`].
///
/// This type represents a path where the caller has indicated that the path
/// should be treated as a file. No filesystem check is performed - the path
/// may or may not exist, and if it exists, may or may not be an actual file.
/// Use [`AbsPathBuf::into_assume_file`] to create instances of this type.
///
/// # Invariants
///
/// An `AbsPresumedFilePathBuf` always contains an absolute path.
#[derive(Clone, Hash, Eq, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct AbsPresumedFilePathBuf(AbsPathBuf);

impl AbsPresumedFilePathBuf {
    /// Creates a new `AbsPresumedFilePathBuf` without checking if the path is
    /// absolute or a file.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the path is absolute.
    #[inline]
    pub unsafe fn new_unchecked(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        debug_assert!(path.is_absolute(), "path must be absolute");
        // SAFETY: The caller guarantees the path is absolute
        Self(unsafe { AbsPathBuf::new_unchecked(path) })
    }

    /// Returns the underlying standard library [`Path`] reference.
    #[inline]
    pub fn as_std_path(&self) -> &Path {
        self.0.as_std_path()
    }

    /// Returns a borrowed [`AbsPresumedFilePath`].
    #[inline]
    pub fn as_path(&self) -> &AbsPresumedFilePath {
        // SAFETY: We maintain the invariant that self.0 is always an absolute file
        unsafe { AbsPresumedFilePath::new_unchecked(self.0.as_std_path()) }
    }

    /// Returns a reference to the inner [`AbsPathBuf`].
    #[inline]
    pub fn as_absolute_path_buf(&self) -> &AbsPathBuf {
        &self.0
    }

    /// Consumes this and returns the inner [`AbsPathBuf`].
    #[inline]
    pub fn into_absolute_path_buf(self) -> AbsPathBuf {
        self.0
    }

    /// Consumes this and returns the inner [`PathBuf`].
    #[inline]
    pub fn into_std_path_buf(self) -> PathBuf {
        self.0.into_std_path_buf()
    }

    /// Returns a normalized version of this path, removing `.` and `..` components,
    /// while retaining the presumption that this is a file path.
    ///
    /// # Errors
    ///
    /// Returns [`NormalizeError::EscapesRoot`] if the path contains too many `..` components
    /// that would escape the root directory.
    pub fn normalized(&self) -> Result<AbsPresumedFilePathBuf, NormalizeError> {
        self.as_path().normalized()
    }
}

impl Deref for AbsPresumedFilePathBuf {
    type Target = AbsPresumedFilePath;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_path()
    }
}

impl AsRef<Path> for AbsPresumedFilePathBuf {
    #[inline]
    fn as_ref(&self) -> &Path {
        self.0.as_std_path()
    }
}

impl AsRef<AbsPath> for AbsPresumedFilePathBuf {
    #[inline]
    fn as_ref(&self) -> &AbsPath {
        self.0.as_path()
    }
}

impl AsRef<AbsPresumedFilePath> for AbsPresumedFilePathBuf {
    #[inline]
    fn as_ref(&self) -> &AbsPresumedFilePath {
        self.as_path()
    }
}

impl Borrow<AbsPresumedFilePath> for AbsPresumedFilePathBuf {
    #[inline]
    fn borrow(&self) -> &AbsPresumedFilePath {
        self.as_path()
    }
}

impl From<AbsPresumedFilePathBuf> for PathBuf {
    #[inline]
    fn from(path: AbsPresumedFilePathBuf) -> Self {
        path.0.into()
    }
}

impl From<AbsPresumedFilePathBuf> for AbsPathBuf {
    #[inline]
    fn from(path: AbsPresumedFilePathBuf) -> Self {
        path.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use tempfile::TempDir;

    fn get_test_dir() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    #[cfg(unix)]
    fn get_nonexistent_absolute_path() -> PathBuf {
        PathBuf::from("/nonexistent_path_12345")
    }

    #[cfg(windows)]
    fn get_nonexistent_absolute_path() -> PathBuf {
        PathBuf::from("C:\\nonexistent_path_12345")
    }

    #[cfg(unix)]
    fn get_absolute_path() -> PathBuf {
        PathBuf::from("/usr")
    }

    #[cfg(windows)]
    fn get_absolute_path() -> PathBuf {
        PathBuf::from("C:\\Windows")
    }

    fn get_relative_path() -> PathBuf {
        PathBuf::from("relative/path")
    }

    // ==================== AbsPath Tests ====================

    #[test]
    fn test_abs_path_new_with_absolute() {
        let path = get_absolute_path();
        let result = AbsPath::new(&path);
        assert!(result.is_ok());
    }

    #[test]
    fn test_abs_path_new_with_relative() {
        let path = get_relative_path();
        let result = AbsPath::new(&path);
        assert!(matches!(result, Err(PathError::NotAbsolute(_))));
    }

    #[test]
    fn test_abs_path_as_std_path() {
        let path = get_absolute_path();
        let abs = AbsPath::new(&path).unwrap();
        assert_eq!(abs.as_std_path(), &path);
        assert!(abs.as_std_path().is_absolute());
    }

    #[test]
    fn test_abs_path_to_path_buf() {
        let path = get_absolute_path();
        let abs = AbsPath::new(&path).unwrap();
        let owned = abs.to_path_buf();
        assert_eq!(owned.as_std_path(), &path);
    }

    #[test]
    fn test_abs_path_directory_on_directory() {
        let temp = get_test_dir();
        let abs = AbsPath::new(temp.path()).unwrap();
        let dir = abs.directory().unwrap();
        assert_eq!(dir.as_std_path(), temp.path());
    }

    #[test]
    fn test_abs_path_directory_on_file() {
        let temp = get_test_dir();
        let file_path = temp.path().join("test_file.txt");
        fs_err::write(&file_path, "test").unwrap();

        let abs = AbsPath::new(&file_path).unwrap();
        let dir = abs.directory().unwrap();
        assert_eq!(dir.as_std_path(), temp.path());
    }

    #[test]
    fn test_abs_path_directory_on_nonexistent() {
        let path = get_nonexistent_absolute_path();
        let abs = AbsPath::new(&path).unwrap();
        assert!(abs.directory().is_none());
    }

    #[test]
    fn test_abs_path_parent() {
        let temp = get_test_dir();
        let child_path = temp.path().join("child");
        fs_err::create_dir(&child_path).unwrap();

        let abs = AbsPath::new(&child_path).unwrap();
        let parent = abs.parent().unwrap();
        assert_eq!(parent.as_std_path(), temp.path());
    }

    #[test]
    fn test_abs_path_parent_on_root() {
        #[cfg(unix)]
        let root = PathBuf::from("/");
        #[cfg(windows)]
        let root = PathBuf::from("C:\\");

        let abs = AbsPath::new(&root).unwrap();
        assert!(abs.parent().is_none());
    }

    #[test]
    fn test_abs_path_assume_dir() {
        let path = get_absolute_path();
        let abs = AbsPath::new(&path).unwrap();
        let dir = abs.assume_dir();
        assert_eq!(dir.as_std_path(), &path);
    }

    #[test]
    fn test_abs_path_assume_file() {
        let path = get_absolute_path();
        let abs = AbsPath::new(&path).unwrap();
        let file = abs.assume_file();
        assert_eq!(file.as_std_path(), &path);
    }

    #[test]
    fn test_abs_path_to_owned() {
        let path = get_absolute_path();
        let abs = AbsPath::new(&path).unwrap();
        let owned: AbsPathBuf = abs.to_owned();
        assert_eq!(owned.as_std_path(), abs.as_std_path());
    }

    // ==================== AbsPathBuf Tests ====================

    #[test]
    fn test_abs_path_buf_new_with_absolute() {
        let path = get_absolute_path();
        let result = AbsPathBuf::new(&path);
        assert!(result.is_ok());
    }

    #[test]
    fn test_abs_path_buf_new_with_relative() {
        let result = AbsPathBuf::new("relative/path");
        assert!(matches!(result, Err(PathError::NotAbsolute(_))));
    }

    #[test]
    fn test_abs_path_buf_deref() {
        let path = get_absolute_path();
        let abs_buf = AbsPathBuf::new(&path).unwrap();
        let abs_ref: &AbsPath = &abs_buf;
        assert_eq!(abs_ref.as_std_path(), &path);
    }

    #[test]
    fn test_abs_path_buf_clone() {
        let path = get_absolute_path();
        let abs_buf = AbsPathBuf::new(&path).unwrap();
        let cloned = abs_buf.clone();
        assert_eq!(abs_buf, cloned);
    }

    #[test]
    fn test_abs_path_buf_hash() {
        let path = get_absolute_path();
        let abs_buf = AbsPathBuf::new(&path).unwrap();

        let mut set = HashSet::new();
        set.insert(abs_buf.clone());
        assert!(set.contains(&abs_buf));
    }

    #[test]
    fn test_abs_path_buf_borrow() {
        let path = get_absolute_path();
        let abs_buf = AbsPathBuf::new(&path).unwrap();
        let borrowed: &AbsPath = abs_buf.borrow();
        assert_eq!(borrowed.as_std_path(), abs_buf.as_std_path());
    }

    #[test]
    fn test_abs_path_buf_into_assume_dir() {
        let path = get_absolute_path();
        let abs_buf = AbsPathBuf::new(&path).unwrap();
        let dir_buf = abs_buf.into_assume_dir();
        assert_eq!(dir_buf.as_std_path(), &path);
    }

    #[test]
    fn test_abs_path_buf_into_assume_file() {
        let path = get_absolute_path();
        let abs_buf = AbsPathBuf::new(&path).unwrap();
        let file_buf = abs_buf.into_assume_file();
        assert_eq!(file_buf.as_std_path(), &path);
    }

    #[test]
    fn test_abs_path_buf_as_ref_implementations() {
        let temp = get_test_dir();

        let abs_buf = AbsPathBuf::new(temp.path()).unwrap();
        let _: &Path = abs_buf.as_ref();
        let _: &AbsPath = abs_buf.as_ref();
    }

    // ==================== AbsPresumedDirPath Tests ====================

    #[test]
    fn test_presumed_dir_path_from_assume() {
        let temp = get_test_dir();
        let abs = AbsPath::new(temp.path()).unwrap();
        let dir = abs.assume_dir();
        assert_eq!(dir.as_std_path(), temp.path());
    }

    #[test]
    fn test_presumed_dir_path_as_absolute_path() {
        let temp = get_test_dir();
        let abs = AbsPath::new(temp.path()).unwrap();
        let dir = abs.assume_dir();
        let abs_back: &AbsPath = dir.as_absolute_path();
        assert_eq!(abs_back.as_std_path(), temp.path());
    }

    #[test]
    fn test_presumed_dir_path_to_path_buf() {
        let temp = get_test_dir();
        let abs = AbsPath::new(temp.path()).unwrap();
        let dir = abs.assume_dir();
        let owned = dir.to_path_buf();
        assert_eq!(owned.as_std_path(), temp.path());
    }

    #[test]
    fn test_presumed_dir_path_join() {
        let temp = get_test_dir();
        let abs = AbsPath::new(temp.path()).unwrap();
        let dir = abs.assume_dir();

        let joined = dir.join("subdir/file.txt");
        assert_eq!(joined.as_std_path(), temp.path().join("subdir/file.txt"));
        assert!(joined.as_std_path().is_absolute());
    }

    #[test]
    fn test_presumed_dir_path_to_owned() {
        let temp = get_test_dir();
        let abs = AbsPath::new(temp.path()).unwrap();
        let dir = abs.assume_dir();
        let owned: AbsPresumedDirPathBuf = dir.to_owned();
        assert_eq!(owned.as_std_path(), dir.as_std_path());
    }

    // ==================== AbsPresumedDirPathBuf Tests ====================

    #[test]
    fn test_presumed_dir_path_buf_from_into_assume() {
        let temp = get_test_dir();
        let abs_buf = AbsPathBuf::new(temp.path()).unwrap();
        let dir_buf = abs_buf.into_assume_dir();
        assert_eq!(dir_buf.as_std_path(), temp.path());
    }

    #[test]
    fn test_presumed_dir_path_buf_deref() {
        let temp = get_test_dir();
        let abs_buf = AbsPathBuf::new(temp.path()).unwrap();
        let dir_buf = abs_buf.into_assume_dir();
        let dir_ref: &AbsPresumedDirPath = &dir_buf;
        assert_eq!(dir_ref.as_std_path(), temp.path());
    }

    #[test]
    fn test_presumed_dir_path_buf_clone() {
        let temp = get_test_dir();
        let abs_buf = AbsPathBuf::new(temp.path()).unwrap();
        let dir_buf = abs_buf.into_assume_dir();
        let cloned = dir_buf.clone();
        assert_eq!(dir_buf, cloned);
    }

    #[test]
    fn test_presumed_dir_path_buf_into_absolute_path_buf() {
        let temp = get_test_dir();
        let abs_buf = AbsPathBuf::new(temp.path()).unwrap();
        let dir_buf = abs_buf.into_assume_dir();
        let abs_buf_back = dir_buf.into_absolute_path_buf();
        assert_eq!(abs_buf_back.as_std_path(), temp.path());
    }

    #[test]
    fn test_presumed_dir_path_buf_borrow() {
        let temp = get_test_dir();
        let abs_buf = AbsPathBuf::new(temp.path()).unwrap();
        let dir_buf = abs_buf.into_assume_dir();
        let borrowed: &AbsPresumedDirPath = dir_buf.borrow();
        assert_eq!(borrowed.as_std_path(), dir_buf.as_std_path());
    }

    #[test]
    fn test_presumed_dir_path_buf_as_absolute_path_buf() {
        let temp = get_test_dir();
        let abs_buf = AbsPathBuf::new(temp.path()).unwrap();
        let dir_buf = abs_buf.into_assume_dir();
        let abs_buf_ref: &AbsPathBuf = dir_buf.as_absolute_path_buf();
        assert_eq!(abs_buf_ref.as_std_path(), temp.path());
    }

    #[test]
    fn test_presumed_dir_path_buf_join() {
        let temp = get_test_dir();
        let abs_buf = AbsPathBuf::new(temp.path()).unwrap();
        let dir_buf = abs_buf.into_assume_dir();

        // join is available via Deref to AbsPresumedDirPath
        let joined = dir_buf.join("subdir/file.txt");
        assert_eq!(joined.as_std_path(), temp.path().join("subdir/file.txt"));
    }

    #[test]
    fn test_presumed_dir_path_buf_as_ref_implementations() {
        let temp = get_test_dir();
        let abs_buf = AbsPathBuf::new(temp.path()).unwrap();
        let dir_buf = abs_buf.into_assume_dir();

        let _: &Path = dir_buf.as_ref();
        let _: &AbsPath = dir_buf.as_ref();
        let _: &AbsPresumedDirPath = dir_buf.as_ref();
    }

    // ==================== AbsPresumedFilePath Tests ====================

    #[test]
    fn test_presumed_file_path_from_assume() {
        let temp = get_test_dir();
        let file_path = temp.path().join("test_file.txt");

        let abs = AbsPath::new(&file_path).unwrap();
        let file = abs.assume_file();
        assert_eq!(file.as_std_path(), &file_path);
    }

    #[test]
    fn test_presumed_file_path_as_absolute_path() {
        let temp = get_test_dir();
        let file_path = temp.path().join("test_file.txt");

        let abs = AbsPath::new(&file_path).unwrap();
        let file = abs.assume_file();
        let abs_back: &AbsPath = file.as_absolute_path();
        assert_eq!(abs_back.as_std_path(), &file_path);
    }

    #[test]
    fn test_presumed_file_path_to_path_buf() {
        let temp = get_test_dir();
        let file_path = temp.path().join("test_file.txt");

        let abs = AbsPath::new(&file_path).unwrap();
        let file = abs.assume_file();
        let owned = file.to_path_buf();
        assert_eq!(owned.as_std_path(), &file_path);
    }

    #[test]
    fn test_presumed_file_path_to_owned() {
        let temp = get_test_dir();
        let file_path = temp.path().join("test_file.txt");

        let abs = AbsPath::new(&file_path).unwrap();
        let file = abs.assume_file();
        let owned: AbsPresumedFilePathBuf = file.to_owned();
        assert_eq!(owned.as_std_path(), file.as_std_path());
    }

    // ==================== AbsPresumedFilePathBuf Tests ====================

    #[test]
    fn test_presumed_file_path_buf_from_into_assume() {
        let temp = get_test_dir();
        let file_path = temp.path().join("test_file.txt");

        let abs_buf = AbsPathBuf::new(&file_path).unwrap();
        let file_buf = abs_buf.into_assume_file();
        assert_eq!(file_buf.as_std_path(), &file_path);
    }

    #[test]
    fn test_presumed_file_path_buf_deref() {
        let temp = get_test_dir();
        let file_path = temp.path().join("test_file.txt");

        let abs_buf = AbsPathBuf::new(&file_path).unwrap();
        let file_buf = abs_buf.into_assume_file();
        let file_ref: &AbsPresumedFilePath = &file_buf;
        assert_eq!(file_ref.as_std_path(), &file_path);
    }

    #[test]
    fn test_presumed_file_path_buf_clone() {
        let temp = get_test_dir();
        let file_path = temp.path().join("test_file.txt");

        let abs_buf = AbsPathBuf::new(&file_path).unwrap();
        let file_buf = abs_buf.into_assume_file();
        let cloned = file_buf.clone();
        assert_eq!(file_buf, cloned);
    }

    #[test]
    fn test_presumed_file_path_buf_into_absolute_path_buf() {
        let temp = get_test_dir();
        let file_path = temp.path().join("test_file.txt");

        let abs_buf = AbsPathBuf::new(&file_path).unwrap();
        let file_buf = abs_buf.into_assume_file();
        let abs_buf_back = file_buf.into_absolute_path_buf();
        assert_eq!(abs_buf_back.as_std_path(), &file_path);
    }

    #[test]
    fn test_presumed_file_path_buf_borrow() {
        let temp = get_test_dir();
        let file_path = temp.path().join("test_file.txt");

        let abs_buf = AbsPathBuf::new(&file_path).unwrap();
        let file_buf = abs_buf.into_assume_file();
        let borrowed: &AbsPresumedFilePath = file_buf.borrow();
        assert_eq!(borrowed.as_std_path(), file_buf.as_std_path());
    }

    #[test]
    fn test_presumed_file_path_buf_as_absolute_path_buf() {
        let temp = get_test_dir();
        let file_path = temp.path().join("test_file.txt");

        let abs_buf = AbsPathBuf::new(&file_path).unwrap();
        let file_buf = abs_buf.into_assume_file();
        let abs_buf_ref: &AbsPathBuf = file_buf.as_absolute_path_buf();
        assert_eq!(abs_buf_ref.as_std_path(), &file_path);
    }

    #[test]
    fn test_presumed_file_path_buf_as_ref_implementations() {
        let temp = get_test_dir();
        let file_path = temp.path().join("test_file.txt");

        let abs_buf = AbsPathBuf::new(&file_path).unwrap();
        let file_buf = abs_buf.into_assume_file();

        let _: &Path = file_buf.as_ref();
        let _: &AbsPath = file_buf.as_ref();
        let _: &AbsPresumedFilePath = file_buf.as_ref();
    }

    // ==================== Edge Cases ====================

    #[test]
    fn test_assume_works_on_nonexistent_paths() {
        // The assume_* methods should work on non-existent paths
        // because they represent intent, not filesystem state
        let path = get_nonexistent_absolute_path();
        let abs = AbsPath::new(&path).unwrap();

        // Both assume_dir and assume_file should work
        let _dir = abs.assume_dir();
        let _file = abs.assume_file();
    }

    #[test]
    fn test_into_assume_works_on_nonexistent_paths() {
        let path = get_nonexistent_absolute_path();

        // into_assume_dir
        let abs_buf = AbsPathBuf::new(&path).unwrap();
        let _dir_buf = abs_buf.into_assume_dir();

        // into_assume_file
        let abs_buf = AbsPathBuf::new(&path).unwrap();
        let _file_buf = abs_buf.into_assume_file();
    }
}
