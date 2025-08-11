#![deny(clippy::dbg_macro, clippy::unwrap_used)]

pub mod activation;
pub mod cli;
pub mod diff;
pub mod environment;
mod global;
pub mod lock_file;
mod prompt;
pub(crate) mod repodata;
pub mod task;
pub mod workspace;

mod signals;

pub use lock_file::UpdateLockFileOptions;
pub use workspace::{DependencyType, Workspace, WorkspaceLocator};
