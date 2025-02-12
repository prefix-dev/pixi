#![deny(clippy::dbg_macro, clippy::unwrap_used)]

pub mod activation;
pub mod cli;
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

pub mod build;
mod rlimit;
mod utils;

pub use lock_file::UpdateLockFileOptions;
pub use workspace::{DependencyType, Workspace, WorkspaceLocator};
