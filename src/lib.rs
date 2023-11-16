pub mod cli;
pub mod config;
pub mod consts;
pub mod environment;
pub mod install;
pub mod lock_file;
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

use once_cell::sync::Lazy;
pub use project::Project;
use rattler_networking::retry_policies::ExponentialBackoff;
use rattler_networking::AuthenticatedClient;
use reqwest::Client;

/// The default retry policy employed by pixi.
/// TODO: At some point we might want to make this configurable.
pub fn default_retry_policy() -> ExponentialBackoff {
    ExponentialBackoff::builder().build_with_max_retries(3)
}

/// Returns the default client to use for networking.
pub fn default_client() -> Client {
    static CLIENT: Lazy<Client> = Lazy::new(Default::default);
    CLIENT.clone()
}

/// Returns the default authenticated client to use for rattler authenticated networking.
pub fn default_authenticated_client() -> AuthenticatedClient {
    static CLIENT: Lazy<AuthenticatedClient> =
        Lazy::new(|| AuthenticatedClient::from_client(default_client(), Default::default()));
    CLIENT.clone()
}
