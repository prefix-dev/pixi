use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

use miette::{Diagnostic, NamedSource, SourceCode};
use pixi_consts::consts;
use thiserror::Error;
use toml_edit::DocumentMut;

use crate::{
    manifests::source::ManifestSource, toml::TomlDocument, utils::WithSourceCode, ManifestDocument,
    TomlError,
};

/// Describes the origin of a manifest file. It contains the location of the
/// manifest on disk, the contents of the file on disk, and the parsed TOML.
#[derive(Debug, Clone)]
pub struct ManifestProvenance {
    /// The path to the manifest file
    pub path: PathBuf,

    /// The source text of the file.
    ///
    /// Note that this is the original source code of the manifest file.
    /// Although `document` was originally created from the contents of this
    /// field, it may have been modified since then.
    ///
    /// This value is updated when calling [`ManifestProvenance::save`].
    pub source: Arc<str>,

    /// The parsed representation of the manifest.
    /// TODO: Remove this and only load it when needed?
    pub document: ManifestDocument,
}

/// An error that is returned when trying to parse a manifest file.
#[derive(Debug, Error, Diagnostic)]
pub enum ProvenanceError {
    /// Returned when reading the source from disk fails.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// Returned when parsing the contents of the file from disk fails.
    #[error(transparent)]
    #[diagnostic(transparent)]
    Toml(#[from] WithSourceCode<TomlError, NamedSource<Arc<str>>>),

    /// Returned when the manifest file format is not recognized.
    #[error("unrecognized manifest file format. Expected either pixi.toml or pyproject.toml.")]
    UnrecognizedManifestFormat,
}

impl ManifestProvenance {
    /// Load the manifest from a path
    pub fn from_path(path: PathBuf) -> Result<Self, ProvenanceError> {
        let Some(file_name) = path.file_name().and_then(OsStr::to_str) else {
            return Err(ProvenanceError::UnrecognizedManifestFormat);
        };
        let Some(manifest_kind) = ManifestKind::try_from_path(&path) else {
            return Err(ProvenanceError::UnrecognizedManifestFormat);
        };

        // Read the contents of the file from disk
        let contents = Arc::from(fs_err::read_to_string(&path)?.as_str());

        // Parse as a TOML document
        let document = DocumentMut::from_str(&contents)
            .map_err(|e| WithSourceCode {
                error: TomlError::from(e),
                source: NamedSource::new(file_name, contents.clone()).with_language("toml"),
            })
            .map(TomlDocument::new)?;

        // Create the manifest source based on the type of manifest
        let source = match manifest_kind {
            ManifestKind::Pixi => ManifestDocument::PixiToml(document),
            ManifestKind::Pyproject => ManifestDocument::PyProjectToml(document),
        };

        Ok(Self {
            path,
            source: contents,
            document: source,
        })
    }

    /// Load the manifest from data already in memory.
    pub fn from_source<S: SourceCode + AsRef<str> + Into<Arc<str>> + 'static>(
        source: ManifestSource<S>,
    ) -> Result<Self, ProvenanceError> {
        // Parse as a TOML document
        let document = match DocumentMut::from_str(source.as_ref().as_ref()) {
            Ok(doc) => TomlDocument::new(doc),
            Err(e) => {
                return Err(WithSourceCode {
                    error: TomlError::from(e),
                    source: NamedSource::new(source.file_name(), source.into_inner().into()),
                }
                .into())
            }
        };

        // Create the manifest source based on the type of manifest
        let document = match &source {
            ManifestSource::PyProjectToml(_) => ManifestDocument::PyProjectToml(document),
            ManifestSource::PixiToml(_) => ManifestDocument::PixiToml(document),
        };

        Ok(Self {
            path: PathBuf::from(source.file_name()),
            source: source.into_inner().into(),
            document,
        })
    }

    /// Write the manifest back to disk. This will overwrite the original file.
    /// Calling this function will also update the `contents` field with the new
    /// source.
    pub fn save(&mut self) -> Result<(), std::io::Error> {
        let contents = self.document.to_string();
        fs_err::write(&self.path, &contents)?;
        self.source = Arc::from(contents.as_str());
        Ok(())
    }

    /// Returns the kind of manifest this instance represents.
    pub fn kind(&self) -> ManifestKind {
        match self.document {
            ManifestDocument::PixiToml(_) => ManifestKind::Pixi,
            ManifestDocument::PyProjectToml(_) => ManifestKind::Pyproject,
        }
    }

    /// Returns a named source for the manifest which can be used to display
    /// errors.
    pub fn named_source(&self) -> NamedSource<Arc<str>> {
        let name = match self.document {
            ManifestDocument::PixiToml(_) => consts::PROJECT_MANIFEST,
            ManifestDocument::PyProjectToml(_) => consts::PROJECT_MANIFEST,
        };
        NamedSource::new(name, self.source.clone()).with_language("toml")
    }
}

#[derive(Debug, Clone)]
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
