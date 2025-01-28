use crate::ManifestSource;
use miette::Diagnostic;
use pixi_consts::consts;
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};
use thiserror::Error;

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
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ManifestKind {
    Pixi,
    Pyproject,
}

impl ManifestKind {
    /// Try to determine the type of manifest from a path
    pub fn try_from_path(path: &Path) -> Option<Self> {
        match path.file_name().and_then(OsStr::to_str)? {
            consts::PROJECT_MANIFEST => Some(Self::Pixi),
            consts::PYPROJECT_MANIFEST => Some(Self::Pyproject),
            _ => None,
        }
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
