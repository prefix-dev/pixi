//! This module contains everything which is related to a Pixi workspace.

pub(crate) mod add;
pub use add::{DependencyOptions, GitOptions};

pub(crate) mod remove;

pub(crate) mod init;
pub use init::{GitAttributes, InitOptions, ManifestFormat};

pub(crate) mod reinstall;
pub use reinstall::ReinstallOptions;

pub(crate) mod search;

pub(crate) mod task;

#[allow(clippy::module_inception)]
pub(crate) mod workspace;
pub use workspace::channel::ChannelOptions;

pub(crate) mod registry;
pub use registry::WorkspaceRegistry;
