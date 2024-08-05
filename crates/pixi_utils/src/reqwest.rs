use std::{path::PathBuf, sync::Arc, time::Duration};

use rattler_networking::{
    authentication_storage::{self, backends::file::FileStorageError},
    mirror_middleware::Mirror,
    retry_policies::ExponentialBackoff,
    AuthenticationMiddleware, AuthenticationStorage, MirrorMiddleware, OciMiddleware,
};

use reqwest::Client;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use std::collections::HashMap;

use pixi_config::Config;

/// The default retry policy employed by pixi.
/// TODO: At some point we might want to make this configurable.
pub fn default_retry_policy() -> ExponentialBackoff {
    ExponentialBackoff::builder().build_with_max_retries(3)
}

fn auth_middleware(config: &Config) -> Result<AuthenticationMiddleware, FileStorageError> {
    if let Some(auth_file) = config.authentication_override_file() {
        tracing::info!("Loading authentication from file: {:?}", auth_file);

        if !auth_file.exists() {
            tracing::warn!("Authentication file does not exist: {:?}", auth_file);
        }

        let mut store = AuthenticationStorage::new();
        store.add_backend(Arc::from(
            authentication_storage::backends::file::FileStorage::new(PathBuf::from(&auth_file))?,
        ));

        return Ok(AuthenticationMiddleware::new(store));
    }

    Ok(AuthenticationMiddleware::default())
}

pub fn mirror_middleware(config: &Config) -> MirrorMiddleware {
    let mut internal_map = HashMap::new();
    tracing::info!("Using mirrors: {:?}", config.mirror_map());

    fn ensure_trailing_slash(url: &url::Url) -> url::Url {
        if url.path().ends_with('/') {
            url.clone()
        } else {
            // Do not use `join` because it removes the last element
            format!("{}/", url)
                .parse()
                .expect("Failed to add trailing slash to URL")
        }
    }

    for (key, value) in config.mirror_map() {
        let mut mirrors = Vec::new();
        for v in value {
            mirrors.push(Mirror {
                url: ensure_trailing_slash(v),
                no_jlap: false,
                no_bz2: false,
                no_zstd: false,
                max_failures: None,
            });
        }
        internal_map.insert(ensure_trailing_slash(key), mirrors);
    }

    MirrorMiddleware::from_map(internal_map)
}

pub fn oci_middleware() -> OciMiddleware {
    OciMiddleware
}

pub fn build_reqwest_clients(config: Option<&Config>) -> (Client, ClientWithMiddleware) {
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
        .read_timeout(Duration::from_secs(timeout))
        .build()
        .expect("failed to create reqwest Client");

    let mut client_builder = ClientBuilder::new(client.clone());

    if !config.mirror_map().is_empty() {
        client_builder = client_builder
            .with(mirror_middleware(&config))
            .with(oci_middleware());
    }

    client_builder = client_builder.with_arc(Arc::new(
        auth_middleware(&config).expect("could not create auth middleware"),
    ));

    let authenticated_client = client_builder.build();

    (client, authenticated_client)
}
