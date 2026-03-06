//! Collection of traits for package specifications and project models
//!
//! The main entry point is the [`ProjectModel`] trait which defines the core
//! interface for a project model.
//!
//! Any backend that will deal with Project (from pixi frontend as example)
//! should implement this.
#![deny(missing_docs)]

pub mod package_spec;
pub mod project;
pub mod targets;

pub use package_spec::{AnyVersion, BinarySpecExt, PackageSourceSpec, PackageSpec};
pub use project::ProjectModel;
pub use targets::{Dependencies, TargetSelector, Targets};
