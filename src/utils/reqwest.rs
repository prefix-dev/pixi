use std::{sync::Arc, time::Duration};

use rattler_networking::{retry_policies::ExponentialBackoff, AuthenticationMiddleware};
use reqwest::Client;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};

use crate::config::Config;

/// The default retry policy employed by pixi.
/// TODO: At some point we might want to make this configurable.
pub fn default_retry_policy() -> ExponentialBackoff {
    ExponentialBackoff::builder().build_with_max_retries(3)
}

pub(crate) fn build_reqwest_clients(config: Option<&Config>) -> (Client, ClientWithMiddleware) {
    static APP_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"),);

    // If we do not have a config, we will just load the global default.
    let config = if let Some(config) = config {
        config.clone()
    } else {
        Config::load_global()
    };

    let timeout = 5 * 60;
    let client = Client::builder()
        .pool_max_idle_per_host(20)
        .user_agent(APP_USER_AGENT)
        .danger_accept_invalid_certs(config.tls_no_verify())
        .timeout(Duration::from_secs(timeout))
        .build()
        .expect("failed to create reqwest Client");

    let authenticated_client = ClientBuilder::new(client.clone())
        .with_arc(Arc::new(AuthenticationMiddleware::default()))
        .build();

    (client, authenticated_client)
}
