use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

use miette::IntoDiagnostic;
use pixi_auth::{get_auth_challenge_middleware, get_auth_middleware, get_auth_store};
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

#[cfg(any(feature = "native-tls", feature = "rustls"))]
use crate::tls::Certificates;

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
                no_bz2: false,
                no_zstd: false,
                max_failures: None,
            });
        }
        internal_map.insert(ensure_trailing_slash(key), mirrors);
    }

    MirrorMiddleware::from_map(internal_map)
}

pub fn oci_middleware(client: LazyReqwestClient) -> OciMiddleware {
    let middleware = LazyClient::new(|| ClientWithMiddleware::new(client.into_client(), vec![]));
    OciMiddleware::new(middleware)
}

static DEFAULT_REQWEST_USER_AGENT: LazyLock<String> =
    LazyLock::new(|| format!("pixi/{}", consts::PIXI_VERSION));
static DEFAULT_REQWEST_TIMEOUT_SEC: Duration = Duration::from_secs(5 * 60);
static DEFAULT_REQWEST_IDLE_PER_HOST: usize = 20;

/// The default `TlsRootCerts` mode for the active TLS backend.
///
/// On `native-tls` builds pixi's own client always talks to the OS trust store,
/// so the default mirrors that with `System`. On `rustls` builds the
/// bundled Mozilla roots are portable and work without any platform integration,
/// so the default is `Webpki`. Users can still override explicitly via
/// `tls-root-certs` in their config.
pub const fn default_tls_root_certs() -> pixi_config::TlsRootCerts {
    #[cfg(feature = "native-tls")]
    {
        pixi_config::TlsRootCerts::System
    }
    #[cfg(not(feature = "native-tls"))]
    {
        pixi_config::TlsRootCerts::Webpki
    }
}

/// Resolve the effective `TlsRootCerts` mode for a given config.
///
/// Falls back to [`default_tls_root_certs`] when the user has not set the field.
fn resolve_tls_root_certs(config: Option<&Config>) -> pixi_config::TlsRootCerts {
    config
        .and_then(Config::tls_root_certs)
        .unwrap_or_else(default_tls_root_certs)
}

/// Whether uv's reqwest client should load the platform's system root certificates.
///
/// uv 0.11 only supports rustls and exposes a single `with_system_certs(bool)` knob:
/// `true` -> let rustls-platform-verifier / `SSL_CERT_FILE`/`SSL_CERT_DIR` provide trust,
/// `false` -> fall back to the bundled Mozilla webpki roots.
///
/// Mirrors pixi's resolved [`pixi_config::TlsRootCerts`]: only `System`
/// (and the deprecated `LegacyNative` alias) maps to `true`. The deprecated
/// `All` mode falls through to `false`; see `load_root_certificates` for the
/// runtime warning.
#[allow(deprecated)]
pub fn should_use_system_certs_for_uv(config: &Config) -> bool {
    matches!(
        resolve_tls_root_certs(Some(config)),
        pixi_config::TlsRootCerts::System | pixi_config::TlsRootCerts::LegacyNative
    )
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

    #[cfg_attr(
        not(any(feature = "native-tls", feature = "rustls")),
        allow(unused_variables)
    )]
    let tls_root_certs = resolve_tls_root_certs(config);

    // rustls has no OS trust store, so it needs explicit anchors via
    // `tls_certs_only`. native-tls already uses the OS store; routing System
    // mode through `tls_certs_only` sets `disable_built_in_roots` and rejects
    // enterprise/proxy CAs the OS trusts (issue #6229), so we keep the OS store
    // and only merge env roots there.
    #[cfg(feature = "native-tls")]
    {
        builder = builder.use_native_tls();
        builder = apply_native_tls_roots(builder, tls_root_certs);
    }
    #[cfg(feature = "rustls")]
    {
        builder = builder.use_rustls_tls();
        builder = builder.tls_certs_only(Certificates::for_mode(tls_root_certs).to_reqwest_certs());
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

/// Configure root certificates for the native-tls backend.
///
/// System keeps the OS store and merges any `SSL_CERT_FILE`/`SSL_CERT_DIR`
/// roots. Webpki replaces the OS store with the bundled Mozilla roots.
#[cfg(feature = "native-tls")]
#[allow(deprecated)]
fn apply_native_tls_roots(
    mut builder: reqwest::ClientBuilder,
    mode: pixi_config::TlsRootCerts,
) -> reqwest::ClientBuilder {
    match mode {
        pixi_config::TlsRootCerts::Webpki => {
            let mut certs = Certificates::webpki_roots();
            if let Some(env_certs) = Certificates::from_env() {
                certs.merge(env_certs);
            }
            builder.tls_certs_only(certs.to_reqwest_certs())
        }
        pixi_config::TlsRootCerts::System
        | pixi_config::TlsRootCerts::LegacyNative
        | pixi_config::TlsRootCerts::All => {
            if let Some(env_certs) = Certificates::from_env() {
                for cert in env_certs.to_reqwest_certs() {
                    builder = builder.add_root_certificate(cert);
                }
            }
            builder
        }
    }
}

pub fn build_reqwest_middleware_stack(
    config: &Config,
    client: &LazyReqwestClient,
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

    // The mirror middleware is only needed when mirrors are configured.
    if !config.mirror_map().is_empty() {
        result.push(Arc::new(mirror_middleware(config)));
    }

    // The OCI middleware rewrites `oci://` requests into real registry requests
    // and is a no-op for other URL schemes. It must be installed unconditionally
    // so that `oci://` channels work even without a mirror configured.
    result.push(Arc::new(oci_middleware(client.clone())));

    result.push(Arc::new(GCSMiddleware::default()));

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

    // Reacts to `WWW-Authenticate` challenges
    result.push(Arc::new(get_auth_challenge_middleware()));

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

    let lazy_client = LazyReqwestClient::new(&config)?;
    let middleware = build_reqwest_middleware_stack(&config, &lazy_client, s3_config_project)?;

    let client = lazy_client.into_client();
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
    let middleware_stack = build_reqwest_middleware_stack(&config, &client, s3_config_project)?;

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

pub fn uv_middlewares(config: &Config, client: LazyReqwestClient) -> Vec<Arc<dyn Middleware>> {
    let mut middlewares: Vec<Arc<dyn Middleware>> = if config.mirror_map().is_empty() {
        vec![]
    } else {
        vec![
            Arc::new(mirror_middleware(config)),
            Arc::new(oci_middleware(client.clone())),
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

        let client = LazyReqwestClient::new(&config).unwrap();
        let middlewares = uv_middlewares(&config, client);

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
        let client = LazyReqwestClient::new(&config).unwrap();
        let middlewares = uv_middlewares(&config, client);

        // Should have: auth middleware only
        assert_eq!(
            middlewares.len(),
            1,
            "Expected exactly 1 middleware (auth) when no mirrors configured, got {}",
            middlewares.len()
        );
    }
}

/// Behavioral tests for the auth-challenge middleware composed in pixi's
/// production order (Authentication then AuthChallenge).
///
/// These drive a real local HTTP server through a stack mirroring
/// `build_reqwest_middleware_stack`'s tail. A test-only [`StubFlow`] stands in
/// for the production `PrefixAuthAmbientFlow`
#[cfg(test)]
mod challenge_tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use pixi_auth::get_auth_middleware;
    use pixi_config::Config;
    use rattler_networking::{
        AuthChallengeMiddleware, AuthFlow, AuthFlowError, BearerToken, Challenge,
    };
    use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
    use url::Url;

    /// An [`AuthFlow`] that always returns a fixed token and counts how often
    /// it is consulted.
    #[derive(Debug)]
    struct StubFlow {
        token: String,
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl AuthFlow for StubFlow {
        async fn acquire_token(
            &self,
            _url: &Url,
            _challenges: &[Challenge],
        ) -> Result<Option<BearerToken>, AuthFlowError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Some(BearerToken::new(self.token.clone())))
        }
    }

    /// Spawn a server that mimics prefix.dev's private-channel behavior:
    /// answer `403` + `WWW-Authenticate: Bearer` until a request carries the
    /// expected bearer token, then `200`. Counts every request received.
    async fn spawn_challenge_server(accept_token: String, hits: Arc<AtomicUsize>) -> String {
        use axum::{
            http::{HeaderMap, StatusCode},
            response::IntoResponse,
            routing::get,
        };

        let app = axum::Router::new().route(
            "/private/repodata.json",
            get(move |headers: HeaderMap| {
                let hits = hits.clone();
                let expected = format!("Bearer {accept_token}");
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    match headers.get("authorization").and_then(|v| v.to_str().ok()) {
                        Some(auth) if auth == expected => (StatusCode::OK, "ok").into_response(),
                        _ => (
                            // prefix.dev returns 403 (not 401) for anonymous
                            // private reads; the middleware reacts to the
                            // challenge header regardless of status.
                            StatusCode::FORBIDDEN,
                            [("www-authenticate", r#"Bearer realm="prefix.dev""#)],
                            "forbidden",
                        )
                            .into_response(),
                    }
                }
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    /// Build a client whose middleware tail matches production:
    /// Authentication (empty store) followed by AuthChallenge.
    fn client_with_challenge(flow: Arc<StubFlow>) -> ClientWithMiddleware {
        let auth = get_auth_middleware(&Config::default()).unwrap();
        ClientBuilder::new(reqwest::Client::new())
            .with_arc(Arc::new(auth))
            .with_arc(Arc::new(AuthChallengeMiddleware::new(vec![flow])))
            .build()
    }

    #[tokio::test]
    #[ignore = "hits beta.prefix.dev private channel; run manually with --ignored"]
    async fn beta_private_channel_drives_production_stack() {
        // Build pixi's REAL production client (full middleware stack incl.
        // get_auth_challenge_middleware() with the default PrefixAuthAmbientFlow)
        // and hit a known private channel on beta.
        //
        // Run with a throwaway ambient identity so the flow proceeds all the
        // way to beta's mint endpoint:
        //   GITLAB_CI=true PREFIX_DEV_ID_TOKEN=fake \
        //   RUST_LOG=rattler_networking=trace \
        //   cargo test -p pixi_utils --lib beta_private_channel_drives_production_stack \
        //     -- --ignored --nocapture
        //
        // Expected: the challenge is detected, PrefixAuthAmbientFlow engages
        // (host ends with .prefix.dev), ambient-id yields the fake token, the
        // flow POSTs https://beta.prefix.dev/api/oidc/mint_token, beta rejects
        // the bogus token, the flow declines, and the original 401 is returned.
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_test_writer()
            .try_init();

        // Drive the FULL production stack (Retry → Mirror → OCI → GCS → S3 →
        // Authentication → AuthChallenge). AuthenticationMiddleware touches the
        // macOS Keychain; approve the prompt once.
        let (_plain, client) = super::build_reqwest_clients(None, None).unwrap();
        let resp = client
            .get("https://beta.prefix.dev/baszalmstra/noarch/repodata.json")
            .send()
            .await
            .unwrap();
        println!("BETA PROBE final status = {}", resp.status());
    }

    #[tokio::test]
    #[ignore = "hits beta.prefix.dev; needs a real channel:read bearer in BETA_TEST_BEARER"]
    async fn beta_private_channel_challenge_replay_with_real_bearer() {
        // Proves challenge detection + replay against live beta to a real 200:
        // beta answers 401 + WWW-Authenticate, the challenge middleware consults
        // the flow (which injects a real channel:read bearer), and replays the
        // request once. Token comes from the env so it never lands in source.
        //   BETA_TEST_BEARER=<pfx-...> \
        //   cargo test -p pixi_utils --lib beta_private_channel_challenge_replay_with_real_bearer \
        //     -- --ignored --nocapture
        let Ok(token) = std::env::var("BETA_TEST_BEARER") else {
            eprintln!("BETA_TEST_BEARER not set; skipping");
            return;
        };
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_test_writer()
            .try_init();

        let flow = Arc::new(StubFlow {
            token,
            calls: AtomicUsize::new(0),
        });
        let client = ClientBuilder::new(reqwest::Client::new())
            .with_arc(Arc::new(AuthChallengeMiddleware::new(vec![flow.clone()])))
            .build();

        let resp = client
            .get("https://beta.prefix.dev/jora/noarch/repodata.json")
            .send()
            .await
            .unwrap();
        let status = resp.status();
        println!(
            "BETA REAL-BEARER status = {status}, flow consulted {} time(s)",
            flow.calls.load(Ordering::SeqCst)
        );
        assert_eq!(
            status, 200,
            "the challenge should be answered and the request replayed to a successful read"
        );
        assert_eq!(
            flow.calls.load(Ordering::SeqCst),
            1,
            "the flow should be consulted exactly once, on the challenge"
        );
    }

    #[tokio::test]
    async fn challenge_on_403_triggers_mint_and_replay() {
        let hits = Arc::new(AtomicUsize::new(0));
        let base = spawn_challenge_server("minted-token".to_string(), hits.clone()).await;
        let flow = Arc::new(StubFlow {
            token: "minted-token".to_string(),
            calls: AtomicUsize::new(0),
        });
        let client = client_with_challenge(flow.clone());

        let response = client
            .get(format!("{base}/private/repodata.json"))
            .send()
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            200,
            "403 challenge should be answered and the request replayed with the bearer token"
        );
        assert_eq!(
            flow.calls.load(Ordering::SeqCst),
            1,
            "the auth flow should be consulted exactly once"
        );
        assert_eq!(
            hits.load(Ordering::SeqCst),
            2,
            "server should see the original challenged request plus one replay"
        );
    }

    #[tokio::test]
    async fn existing_authorization_header_skips_the_challenge_flow() {
        // Stored credentials win: a request that already carries an
        // `Authorization` header is passed straight through and the flow is
        // never consulted (the contract that makes the Auth-before-Challenge
        // ordering safe).
        let hits = Arc::new(AtomicUsize::new(0));
        let base = spawn_challenge_server("minted-token".to_string(), hits.clone()).await;
        let flow = Arc::new(StubFlow {
            token: "should-not-be-used".to_string(),
            calls: AtomicUsize::new(0),
        });
        let client = client_with_challenge(flow.clone());

        let response = client
            .get(format!("{base}/private/repodata.json"))
            .bearer_auth("minted-token")
            .send()
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            200,
            "preset credentials should be accepted"
        );
        assert_eq!(
            flow.calls.load(Ordering::SeqCst),
            0,
            "the challenge flow must not run when Authorization is already present"
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1, "no challenge, so no replay");
    }
}
