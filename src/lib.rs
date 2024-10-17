pub mod activation;
pub mod cli;
pub(crate) mod conda_pypi_clobber;
pub mod environment;
mod global;
mod install_pypi;
mod install_wheel;
mod lock_file;
mod prefix;
mod project;
mod prompt;
pub(crate) mod repodata;
pub mod task;

mod uv_reporter;

mod build;
mod rlimit;
mod utils;

pub use lock_file::{load_lock_file, UpdateLockFileOptions};
pub use project::{DependencyType, Project};
