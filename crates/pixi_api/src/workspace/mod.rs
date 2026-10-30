//! This module contains everything which is related to a Pixi workspace.

pub mod platforms;

pub(crate) mod add;
pub use add::{DependencyOptions, GitOptions};

pub(crate) mod remove;
pub use remove::RemoveError;

pub(crate) mod init;
pub use init::{GitAttributes, InitError, InitOptions, ManifestFormat};

pub(crate) mod list;
pub use list::{Package, PackageKind};

pub(crate) mod reinstall;
pub use reinstall::ReinstallOptions;

pub(crate) mod run;
pub use run::{RunOptions, run_future_forwarding_signals};

pub(crate) mod search;

pub(crate) mod shell;
pub use shell::{ShellError, ShellOptions};

pub(crate) mod task;

#[allow(clippy::module_inception)]
pub(crate) mod workspace;
pub use workspace::activation::ActivationEntry;
pub use workspace::channel::ChannelOptions;
