mod activation;
pub mod cli;
mod config;
pub mod consts;
mod environment;
mod install;
mod install_pypi;
mod lock_file;
mod prefix;
mod progress;
mod project;
mod prompt;
mod repodata;
pub mod task;
#[cfg(unix)]
pub mod unix;
pub mod util;
pub mod utils;

mod pypi_marker_env;
mod pypi_tags;

pub use activation::get_activation_env;
pub use environment::UpdateLockFileOptions;
pub use lock_file::load_lock_file;
pub use project::{
    manifest::{EnvironmentName, FeatureName},
    DependencyType, Project, SpecType,
};
pub use task::{
    CmdArgs, ExecutableTask, FindTaskError, FindTaskSource, RunOutput, SearchEnvironments, Task,
    TaskDisambiguation, TaskExecutionError, TaskGraph, TaskGraphError,
};

use rattler_networking::retry_policies::ExponentialBackoff;

/// The default retry policy employed by pixi.
/// TODO: At some point we might want to make this configurable.
pub fn default_retry_policy() -> ExponentialBackoff {
    ExponentialBackoff::builder().build_with_max_retries(3)
}
