pub mod cli;
pub mod config;
pub mod consts;
pub mod environment;
pub mod prefix;
pub mod progress;
pub mod project;
mod prompt;
pub mod repodata;
pub mod task;
#[cfg(unix)]
pub mod unix;
pub mod util;
pub mod utils;
pub mod virtual_packages;

pub use project::Project;
use rattler_networking::retry_policies::ExponentialBackoff;

/// The default retry policy employed by pixi.
/// TODO: At some point we might want to make this configurable.
pub fn default_retry_policy() -> ExponentialBackoff {
    ExponentialBackoff::builder().build_with_max_retries(3)
}
