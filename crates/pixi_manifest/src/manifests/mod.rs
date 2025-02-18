//! Manifests are the core of the Pixi system. They are the files that define
//! the structure of a project, and are used to access and manipulate the
//! workspace and package data.
//!
//! The main entry point into the manifest is the [`Manifest`] struct which
//! represents a parsed `pixi.toml`. This struct is used to both access and
//! manipulate the manifest data. It also holds the original source code of the
//! manifest file which allows relating certain parts of the manifest back to
//! the original source code.

pub mod table_name;

mod document;
mod package;
mod provenance;
mod source;
mod workspace;

pub use document::ManifestDocument;
pub use package::PackageManifest;
pub use provenance::{
    AssociateProvenance, ManifestKind, ManifestProvenance, ProvenanceError, WithProvenance,
};
pub use source::ManifestSource;
pub use workspace::{WorkspaceManifest, WorkspaceManifestMut};
