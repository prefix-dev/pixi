use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

use miette::Diagnostic;
use pixi_consts::consts;
use thiserror::Error;

use crate::ManifestSource;

/// Describes the origin of a manifest file.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ManifestProvenance {
    /// The path to the manifest file
    pub path: PathBuf,

    /// The type of manifest
    pub kind: ManifestKind,
}

/// An error that is returned when trying to parse a manifest file.
#[derive(Debug, Error, Diagnostic)]
pub enum ProvenanceError {
    /// Returned when the manifest file format is not recognized.
    #[error("unrecognized manifest file format. Expected either pixi.toml or pyproject.toml.")]
    UnrecognizedManifestFormat,
}

impl ManifestProvenance {
    /// Constructs a new `ManifestProvenance` instance.
    pub fn new(path: PathBuf, kind: ManifestKind) -> Self {
        Self { path, kind }
    }

    /// Modifies the provenance to be relative to the specified directory.
    pub fn relative_to(self, base: &Path) -> Self {
        Self::new(
            pathdiff::diff_paths(&self.path, base).unwrap_or(self.path),
            self.kind,
        )
    }

    /// Load the manifest from a path
    pub fn from_path(path: PathBuf) -> Result<Self, ProvenanceError> {
        let Some(kind) = ManifestKind::try_from_path(&path) else {
            return Err(ProvenanceError::UnrecognizedManifestFormat);
        };

        Ok(Self { kind, path })
    }

    /// Load the contents of the manifest.
    pub fn read(&self) -> Result<ManifestSource<String>, std::io::Error> {
        let contents = fs_err::read_to_string(&self.path)?;
        match self.kind {
            ManifestKind::Pixi => Ok(ManifestSource::PixiToml(contents)),
            ManifestKind::Pyproject => Ok(ManifestSource::PyProjectToml(contents)),
            ManifestKind::MojoProject => Ok(ManifestSource::MojoProjectToml(contents)),
        }
    }

    /// Returns the absolute path to the manifest file.
    ///
    /// This method canonicalizes the parent directory but preserves the original
    /// filename, which allows symlinked manifest files to be treated correctly.
    pub fn absolute_path(&self) -> PathBuf {
        match (self.path.parent(), self.path.file_name()) {
            (Some(parent), Some(file_name)) => dunce::canonicalize(parent)
                .map(|canonical_parent| canonical_parent.join(file_name))
                .unwrap_or_else(|_| self.path.to_path_buf()),
            _ => self.path.to_path_buf(),
        }
    }
}

impl From<ManifestKind> for ManifestProvenance {
    fn from(value: ManifestKind) -> Self {
        ManifestProvenance::new(PathBuf::from(value.file_name()), value)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ManifestKind {
    Pixi,
    Pyproject,
    MojoProject,
}

impl ManifestKind {
    /// Try to determine the type of manifest from a path
    pub fn try_from_path(path: &Path) -> Option<Self> {
        match path.file_name().and_then(OsStr::to_str)? {
            consts::WORKSPACE_MANIFEST => Some(Self::Pixi),
            consts::PYPROJECT_MANIFEST => Some(Self::Pyproject),
            consts::MOJOPROJECT_MANIFEST => Some(Self::MojoProject),
            _ => None,
        }
    }

    /// Returns the default file name for a manifest of a certain kind.
    pub fn file_name(self) -> &'static str {
        match self {
            ManifestKind::Pixi => consts::WORKSPACE_MANIFEST,
            ManifestKind::Pyproject => consts::PYPROJECT_MANIFEST,
            ManifestKind::MojoProject => consts::MOJOPROJECT_MANIFEST,
        }
    }

    /// Returns the language of the manifest file
    pub fn language(self) -> &'static str {
        "toml"
    }
}

/// Binds a value read from a manifest to its provenance.
#[derive(Debug, Clone)]
pub struct WithProvenance<T> {
    /// The value constructed from the provenance.
    pub value: T,

    /// The provenance of the value.
    pub provenance: ManifestProvenance,
}

impl<T> WithProvenance<T> {
    /// Constructs a new `WithProvenance` instance.
    pub fn new(value: T, provenance: ManifestProvenance) -> Self {
        Self { value, provenance }
    }

    /// Maps the value of the `WithProvenance` instance to a new value. The
    /// provenance remains untouched.
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> WithProvenance<U> {
        WithProvenance {
            value: f(self.value),
            provenance: self.provenance,
        }
    }
}

/// A trait to associate a provenance with a value. This has a blanked
/// implementation which allows calling `with_provenance` on any value.
pub trait AssociateProvenance: Sized {
    fn with_provenance(self, provenance: ManifestProvenance) -> WithProvenance<Self>;
}

impl<T> AssociateProvenance for T {
    fn with_provenance(self, provenance: ManifestProvenance) -> WithProvenance<Self> {
        WithProvenance::new(self, provenance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_path_canonicalizes_parent_only() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dotfiles_dir = temp_dir.path().join("dotfiles");
        let home_dir = temp_dir.path().join("home");
        std::fs::create_dir_all(&dotfiles_dir).unwrap();
        std::fs::create_dir_all(&home_dir).unwrap();

        // Real manifest lives inside the dotfiles directory.
        let real_manifest = dotfiles_dir.join("pixi.toml");
        std::fs::write(&real_manifest, "[workspace]\nname = \"test\"\n").unwrap();

        // Home directory contains a symlink that points at the real manifest.
        let symlink_manifest = home_dir.join("pixi.toml");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_manifest, &symlink_manifest).unwrap();

        let canonical_real_path = real_manifest.canonicalize().unwrap();
        let cases = [
            (
                "real file",
                real_manifest.clone(),
                dotfiles_dir.clone(),
                true,
            ),
            (
                "symlinked file",
                symlink_manifest.clone(),
                home_dir.clone(),
                false,
            ),
        ];

        for (label, manifest_path, expected_parent, should_match_real) in cases {
            let provenance = ManifestProvenance::new(manifest_path.clone(), ManifestKind::Pixi);
            let absolute = provenance.absolute_path();

            assert_eq!(
                absolute.file_name(),
                manifest_path.file_name(),
                "filename changed for {}",
                label
            );
            assert_eq!(
                absolute.parent().unwrap(),
                expected_parent.canonicalize().unwrap(),
                "parent directory mismatch for {}",
                label
            );

            if should_match_real {
                assert_eq!(
                    absolute, canonical_real_path,
                    "real file should resolve exactly"
                );
            } else {
                assert_ne!(
                    absolute, canonical_real_path,
                    "symlink should not resolve to the real file path"
                );
            }
        }
    }
}
