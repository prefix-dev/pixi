#![deny(clippy::dbg_macro, clippy::unwrap_used)]

pub mod activation;
pub mod diff;
pub mod environment;
pub mod global;
mod install_pypi;
pub mod lock_file;
pub mod prompt;
pub(crate) mod repodata;
pub mod task;
pub mod workspace;

pub mod signals;

pub use lock_file::UpdateLockFileOptions;
pub use workspace::{DependencyType, Workspace, WorkspaceLocator};
