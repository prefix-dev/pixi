use std::{any::Any, path::PathBuf, sync::Arc, time::Duration};

use miette::IntoDiagnostic;
use pixi_consts::consts;
use rattler_networking::{
    authentication_storage::{self, AuthenticationStorageError},
    mirror_middleware::Mirror,
    retry_policies::ExponentialBackoff,
    AuthenticationMiddleware, AuthenticationStorage, GCSMiddleware, MirrorMiddleware,
    OciMiddleware, S3Middleware,
};

use reqwest::Client;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::RetryTransientMiddleware;
use std::collections::HashMap;
use tracing::debug;

use pixi_config::Config;

/// The default retry policy employed by pixi.
/// TODO: At some point we might want to make this configurable.
pub fn default_retry_policy() -> ExponentialBackoff {
    ExponentialBackoff::builder().build_with_max_retries(3)
}

fn auth_store(config: &Config) -> Result<AuthenticationStorage, AuthenticationStorageError> {
    let mut store = AuthenticationStorage::from_env_and_defaults()?;
    if let Some(auth_file) = config.authentication_override_file() {
        tracing::info!("Loading authentication from file: {:?}", auth_file);

        if !auth_file.exists() {
            tracing::warn!("Authentication file does not exist: {:?}", auth_file);
        }

        // this should be the first place before the keyring authentication
        // i.e. either index 0 if RATTLER_AUTH_FILE is not set or index 1 if it is
        let first_storage = store.backends.first().unwrap();
        let index = if first_storage.type_id()
            == std::any::TypeId::of::<authentication_storage::backends::file::FileStorage>()
        {
            1
        } else {
            0
        };
        store.backends.insert(
            index,
            Arc::from(
                authentication_storage::backends::file::FileStorage::from_path(PathBuf::from(
                    &auth_file,
                ))?,
            ),
        );
    }
    Ok(store)
}

fn auth_middleware(
    config: &Config,
) -> Result<AuthenticationMiddleware, AuthenticationStorageError> {
    Ok(AuthenticationMiddleware::from_auth_storage(auth_store(
        config,
    )?))
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

pub fn build_reqwest_clients(
    config: Option<&Config>,
    s3_config_project: Option<HashMap<String, rattler_networking::s3_middleware::S3Config>>,
) -> miette::Result<(Client, ClientWithMiddleware)> {
    let app_user_agent = format!("pixi/{}", consts::PIXI_VERSION);

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
        .user_agent(app_user_agent)
        .danger_accept_invalid_certs(config.tls_no_verify())
        .read_timeout(Duration::from_secs(timeout))
        .use_rustls_tls()
        .build()
        .expect("failed to create reqwest Client");

    let mut client_builder = ClientBuilder::new(client.clone());

    if !config.mirror_map().is_empty() {
        client_builder = client_builder
            .with(mirror_middleware(&config))
            .with(oci_middleware());
    }

    client_builder = client_builder.with(GCSMiddleware);

    let s3_config_global = config.compute_s3_config();
    let s3_config_project = s3_config_project.unwrap_or_default();
    let mut s3_config = HashMap::new();
    s3_config.extend(s3_config_global);
    s3_config.extend(s3_config_project);

    debug!("Using s3_config: {:?}", s3_config);
    let store = auth_store(&config).into_diagnostic()?;
    let s3_middleware = S3Middleware::new(s3_config, store);
    debug!("s3_middleware: {:?}", s3_middleware);
    client_builder = client_builder.with(s3_middleware);

    client_builder = client_builder.with_arc(Arc::new(
        auth_middleware(&config).expect("could not create auth middleware"),
    ));

    client_builder = client_builder.with(RetryTransientMiddleware::new_with_policy(
        default_retry_policy(),
    ));

    let authenticated_client = client_builder.build();

    Ok((client, authenticated_client))
}
