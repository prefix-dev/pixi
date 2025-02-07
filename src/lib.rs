#![deny(clippy::dbg_macro, clippy::unwrap_used)]

pub mod activation;
pub mod cli;
pub(crate) mod conda_prefix_updater;
pub mod diff;
pub mod environment;
mod global;
mod install_pypi;
pub mod lock_file;
mod prefix;
mod prompt;
pub(crate) mod repodata;
pub mod task;
pub mod workspace;

mod uv_reporter;

mod build;
mod rlimit;
mod utils;

pub use conda_prefix_updater::{CondaPrefixUpdated, CondaPrefixUpdater};
pub use lock_file::{load_lock_file, UpdateLockFileOptions};
pub use workspace::{DependencyType, Workspace, WorkspaceLocator};
