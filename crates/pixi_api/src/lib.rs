pub mod activation;
pub mod cli;
pub mod diff;
pub mod environment;
mod install_pypi;
pub mod lock_file;
mod prefix;
pub(crate) mod repodata;
mod reporters;
mod rlimit;
pub mod task;
mod uv_reporter;
pub mod variants;
pub mod workspace;

pub use lock_file::UpdateLockFileOptions;
pub use workspace::{DependencyType, Workspace, WorkspaceLocator};
