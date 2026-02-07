use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

use miette::IntoDiagnostic;
use pixi_auth::{get_auth_middleware, get_auth_store};
use pixi_config::Config;
use pixi_consts::consts;
use rattler_networking::{
    GCSMiddleware, LazyClient, MirrorMiddleware, OciMiddleware, S3Middleware,
    mirror_middleware::Mirror,
};
use reqwest::Client;
use reqwest_middleware::{ClientWithMiddleware, Middleware};
use reqwest_retry::RetryTransientMiddleware;
use retry_policies::policies::ExponentialBackoff;

/// The default retry policy employed by pixi.
/// TODO: At some point we might want to make this configurable.
pub fn default_retry_policy() -> ExponentialBackoff {
    ExponentialBackoff::builder().build_with_max_retries(3)
}

pub fn mirror_middleware(config: &Config) -> MirrorMiddleware {
    let mut internal_map = HashMap::new();
    tracing::debug!("Using mirrors: {:?}", config.mirror_map());

    fn ensure_trailing_slash(url: &url::Url) -> url::Url {
        if url.path().ends_with('/') {
            url.clone()
        } else {
            // Do not use `join` because it removes the last element
            format!("{url}/")
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

static DEFAULT_REQWEST_USER_AGENT: LazyLock<String> =
    LazyLock::new(|| format!("pixi/{}", consts::PIXI_VERSION));
static DEFAULT_REQWEST_TIMEOUT_SEC: Duration = Duration::from_secs(5 * 60);
static DEFAULT_REQWEST_IDLE_PER_HOST: usize = 20;

/// Returns whether UV should use native TLS (system certificates).
///
/// For `native-tls` builds, this always returns `true` since the system TLS library is used.
/// For `rustls-tls` builds, this returns `true` if the config is set to `Native` or `All`.
pub fn should_use_native_tls_for_uv() -> bool {
    tls_backend() == "native-tls"
}

/// Determines whether we should load all builtin certificates
/// for uv
pub fn should_use_builtin_certs_uv(config: &Config) -> bool {
    matches!(config.tls_root_certs(), pixi_config::TlsRootCerts::All)
}

/// Returns the name of the TLS backend used by this build.
///
/// This is determined at compile time based on the enabled features.
pub fn tls_backend() -> &'static str {
    #[cfg(feature = "native-tls")]
    {
        "native-tls"
    }

    #[cfg(not(feature = "native-tls"))]
    {
        "rustls"
    }
}

pub fn reqwest_client_builder(config: Option<&Config>) -> miette::Result<reqwest::ClientBuilder> {
    let mut builder = Client::builder()
        .pool_max_idle_per_host(DEFAULT_REQWEST_IDLE_PER_HOST)
        .user_agent(DEFAULT_REQWEST_USER_AGENT.as_str())
        .read_timeout(DEFAULT_REQWEST_TIMEOUT_SEC);

    #[cfg(feature = "native-tls")]
    {
        // With native-tls, the system's TLS library handles certificates.
        // The tls-root-certs setting has no effect - warn if it's explicitly set.
        if let Some(tls_root_certs) = config.and_then(|c| c.tls_root_certs) {
            tracing::warn!(
                "tls-root-certs is set to '{}' but has no effect with native-tls builds. \
                 System certificates are always used.",
                tls_root_certs
            );
        }
        builder = builder.use_native_tls();
    }

    #[cfg(feature = "rustls-tls")]
    {
        use pixi_config::TlsRootCerts;
        let tls_root_certs = config.map(|c| c.tls_root_certs()).unwrap_or_default();

        builder = builder.use_rustls_tls().tls_built_in_root_certs(false); // Disable auto-loading to choose explicitly

        match tls_root_certs {
            TlsRootCerts::Webpki => {
                builder = builder.tls_built_in_webpki_certs(true);
            }
            TlsRootCerts::Native => {
                builder = builder.tls_built_in_native_certs(true);
            }
            TlsRootCerts::All => {
                builder = builder
                    .tls_built_in_webpki_certs(true)
                    .tls_built_in_native_certs(true);
            }
        }
    }

    let proxies = config
        .map(|c| c.get_proxies())
        .transpose()
        .into_diagnostic()?
        .unwrap_or_default();

    for p in proxies {
        builder = builder.proxy(p);
    }

    Ok(builder)
}

pub fn build_reqwest_middleware_stack(
    config: &Config,
    s3_config_project: Option<HashMap<String, rattler_networking::s3_middleware::S3Config>>,
) -> miette::Result<Box<[Arc<dyn Middleware>]>> {
    let mut result: Vec<Arc<dyn Middleware>> = Vec::new();

    // Retry middleware must come before mirror middleware so that when a mirror
    // returns a server error (e.g. 500), the retry will go through the mirror
    // middleware again, which will then select a different mirror due to the
    // recorded failure.
    result.push(Arc::new(RetryTransientMiddleware::new_with_policy(
        default_retry_policy(),
    )));

    if !config.mirror_map().is_empty() {
        result.push(Arc::new(mirror_middleware(config)));
        result.push(Arc::new(oci_middleware()));
    }

    result.push(Arc::new(GCSMiddleware));

    let s3_config_global = config.compute_s3_config();
    let s3_config_project = s3_config_project.unwrap_or_default();
    let mut s3_config = HashMap::new();
    s3_config.extend(s3_config_global);
    s3_config.extend(s3_config_project);

    let store = get_auth_store(config).into_diagnostic()?;
    result.push(Arc::new(S3Middleware::new(s3_config, store)));

    result.push(Arc::new(
        get_auth_middleware(config).expect("could not create auth middleware"),
    ));

    Ok(result.into_boxed_slice())
}

pub fn build_reqwest_clients(
    config: Option<&Config>,
    s3_config_project: Option<HashMap<String, rattler_networking::s3_middleware::S3Config>>,
) -> miette::Result<(Client, ClientWithMiddleware)> {
    // If we do not have a config, we will just load the global default.
    let config = if let Some(config) = config {
        Cow::Borrowed(config)
    } else {
        Cow::Owned(Config::load_global())
    };

    let client = LazyReqwestClient::new(&config)?.into_client();
    let middleware = build_reqwest_middleware_stack(&config, s3_config_project)?;
    let authenticated_client = ClientWithMiddleware::new(client.clone(), middleware);

    Ok((client, authenticated_client))
}

pub fn build_lazy_reqwest_clients(
    config: Option<&Config>,
    s3_config_project: Option<HashMap<String, rattler_networking::s3_middleware::S3Config>>,
) -> miette::Result<(LazyReqwestClient, LazyClient)> {
    // If we do not have a config, we will just load the global default.
    let config = if let Some(config) = config {
        Cow::Borrowed(config)
    } else {
        Cow::Owned(Config::load_global())
    };

    let client = LazyReqwestClient::new(&config)?;
    let middleware_stack = build_reqwest_middleware_stack(&config, s3_config_project)?;

    let client_for_middleware = client.clone();
    let client_with_middleware = rattler_networking::LazyClient::new(move || {
        let client = client_for_middleware.into_client();
        ClientWithMiddleware::new(client, middleware_stack)
    });

    Ok((client, client_with_middleware))
}

/// This is a wrapper around reqwest::Client that initializes the client lazily.
///
/// This is useful because the initialization of the client can be expensive.
#[derive(Clone)]
pub struct LazyReqwestClient {
    pub client: Arc<LazyLock<reqwest::Client, Box<dyn FnOnce() -> reqwest::Client + Send + Sync>>>,
}

impl LazyReqwestClient {
    pub fn new(config: &Config) -> miette::Result<Self> {
        let tls_no_verify = config.tls_no_verify();
        if tls_no_verify {
            tracing::warn!(
                "TLS verification is disabled. This is insecure and should only be used for testing or internal networks."
            );
        }

        let builder =
            reqwest_client_builder(Some(config))?.danger_accept_invalid_certs(tls_no_verify);

        Ok(Self {
            client: Arc::new(LazyLock::new(Box::new(move || {
                let start = Instant::now();
                let client = builder
                    .danger_accept_invalid_certs(tls_no_verify)
                    .build()
                    .expect("failed to create reqwest Client");
                tracing::debug!("instantiating reqwest Client took {:?}", start.elapsed());
                client
            }))),
        })
    }

    pub fn into_client(self) -> reqwest::Client {
        (*self.client).clone()
    }
}

pub fn uv_middlewares(config: &Config) -> Vec<Arc<dyn Middleware>> {
    let mut middlewares: Vec<Arc<dyn Middleware>> = if config.mirror_map().is_empty() {
        vec![]
    } else {
        vec![
            Arc::new(mirror_middleware(config)),
            Arc::new(oci_middleware()),
        ]
    };

    // Add authentication middleware after mirror rewriting so it can authenticate
    // against the rewritten URLs (important for mirrors that require different
    // credentials)
    if let Ok(auth_middleware) = get_auth_middleware(config) {
        middlewares.push(Arc::new(auth_middleware));
    }
    middlewares
}

#[cfg(test)]
mod tests {
    use pixi_config::Config;
    use url::Url;

    use super::*;

    #[test]
    fn test_uv_middlewares_includes_auth_with_mirrors() {
        // Test that authentication middleware is included when mirrors are configured
        // This ensures credentials work with rewritten mirror URLs
        let mut config = Config::default();
        config.mirrors.insert(
            Url::parse("https://pypi.org/simple/").unwrap(),
            vec![Url::parse("https://my-mirror.example.com/simple/").unwrap()],
        );

        let middlewares = uv_middlewares(&config);

        // Should have: mirror + OCI + auth middleware
        assert!(
            middlewares.len() >= 3,
            "Expected at least 3 middlewares (mirror, OCI, auth) when mirrors configured, got {}",
            middlewares.len()
        );
    }

    #[test]
    fn test_uv_middlewares_includes_auth_without_mirrors() {
        // Test that authentication middleware is still included even without mirrors
        // This ensures existing non-mirror auth scenarios continue to work
        let config = Config::default();
        let middlewares = uv_middlewares(&config);

        // Should have: auth middleware only
        assert_eq!(
            middlewares.len(),
            1,
            "Expected exactly 1 middleware (auth) when no mirrors configured, got {}",
            middlewares.len()
        );
    }
}
