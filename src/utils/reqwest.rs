use std::{path::PathBuf, sync::Arc, time::Duration};

use rattler_networking::{
    authentication_storage, retry_policies::ExponentialBackoff, AuthenticationMiddleware,
    AuthenticationStorage,
};
use reqwest::Client;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};

use crate::config::Config;

/// The default retry policy employed by pixi.
/// TODO: At some point we might want to make this configurable.
pub fn default_retry_policy() -> ExponentialBackoff {
    ExponentialBackoff::builder().build_with_max_retries(3)
}

fn auth_middleware() -> AuthenticationMiddleware {
    if let Ok(auth_file) = std::env::var("RATTLER_AUTH_FILE") {
        tracing::info!("Loading authentication from file: {:?}", auth_file);

        let mut store = AuthenticationStorage::new();
        store.add_backend(Arc::from(
            authentication_storage::backends::file::FileStorage::new(PathBuf::from(&auth_file)),
        ));

        return AuthenticationMiddleware::new(store);
    }

    AuthenticationMiddleware::default()
}

pub(crate) fn build_reqwest_clients(config: Option<&Config>) -> (Client, ClientWithMiddleware) {
    static APP_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"),);

    // If we do not have a config, we will just load the global default.
    let config = if let Some(config) = config {
        config.clone()
    } else {
        Config::load_global()
    };

    if config.tls_no_verify() {
        tracing::warn!("TLS verification is disabled. This is insecure and should only be used for testing or internal networks.");
    }

    let timeout = 5 * 60;
    let client = Client::builder()
        .pool_max_idle_per_host(20)
        .user_agent(APP_USER_AGENT)
        .danger_accept_invalid_certs(config.tls_no_verify())
        .timeout(Duration::from_secs(timeout))
        .build()
        .expect("failed to create reqwest Client");

    let authenticated_client = ClientBuilder::new(client.clone())
        .with_arc(Arc::new(auth_middleware()))
        .build();

    (client, authenticated_client)
}
