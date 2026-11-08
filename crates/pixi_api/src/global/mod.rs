//! This module contains everything which is related to pixi global.

pub(crate) mod install;
pub use install::{InstallError, InstallOptions};

pub(crate) mod list;
pub use list::{GlobalDependency, GlobalEnvironment, GlobalExposedMapping, ListError};

pub(crate) mod uninstall;
pub use uninstall::UninstallError;

pub(crate) mod utils;
pub use utils::{eventual_environment_channels, revert_environment_after_error};

// Reexport for pixi_api consumers
pub use pixi_global::{EnvironmentName, Mapping, Project, StateChanges, project::GlobalSpec};
