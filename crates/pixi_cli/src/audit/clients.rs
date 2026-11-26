//! HTTP clients for the vulnerability databases and threat feeds, built from
//! the effective Pixi configuration.
//!
//! Basilisk credentials are attached only to the configured Basilisk origin:
//! a Basilisk API token from `PIXI_AUDIT_TOKEN` or the authentication store,
//! or, for the production instance only, the saved `prefix.dev` login when
//! its audience is Basilisk. Redirects are never followed, and OSV, the
//! threat feeds, and mirrors never see these credentials.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use super::error::AuditCommandError;
use http_cache_reqwest::{CACacheManager, Cache, CacheMode, HttpCache, HttpCacheOptions};
use pixi_audit::{
    BASILISK_URL_ENV_VAR, BasiliskClient, DEFAULT_BASILISK_URL, DEFAULT_EPSS_BASE_URL,
    DEFAULT_KEV_URL, DEFAULT_OSV_URL, EPSS_BASE_URL_ENV_VAR, EpssClient, KEV_URL_ENV_VAR,
    KevClient, OSV_URL_ENV_VAR, OsvDevClient, TOKEN_ENV_VAR, looks_like_basilisk_token,
};
use pixi_config::{CacheKind, Config};
use rattler_networking::{
    Authentication, AuthenticationMiddleware, AuthenticationStorage, OfflineMiddleware,
};
use reqwest::{
    Request, Response,
    header::{AUTHORIZATION, HeaderValue},
};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware, Middleware, Next};
use reqwest_retry::RetryTransientMiddleware;
use url::{Origin, Url};

/// The credential key of the shared `prefix.dev` login.
const SHARED_LOGIN_KEY: &str = "prefix.dev";

/// The clients one audit invocation uses.
pub(super) struct AuditClients {
    pub basilisk: BasiliskClient,
    pub osv: OsvDevClient,
    pub epss: EpssClient,
    pub kev: KevClient,
    pub basilisk_url: Url,
    pub osv_url: Url,
}

impl AuditClients {
    pub fn threat_sources(&self) -> pixi_audit::ThreatSources<'_> {
        pixi_audit::ThreatSources {
            epss: Some(&self.epss),
            kev: Some(&self.kev),
        }
    }
}

fn url_from_env(variable: &'static str, default: &str) -> Result<Url, AuditCommandError> {
    let value = std::env::var(variable).unwrap_or_else(|_| default.to_string());
    Url::parse(&value).map_err(|source| AuditCommandError::InvalidUrl {
        variable,
        value,
        source,
    })
}

pub(super) fn build(config: &Config) -> miette::Result<AuditClients> {
    let basilisk_url = url_from_env(BASILISK_URL_ENV_VAR, DEFAULT_BASILISK_URL)?;
    let osv_url = url_from_env(OSV_URL_ENV_VAR, DEFAULT_OSV_URL)?;
    let epss_url = url_from_env(EPSS_BASE_URL_ENV_VAR, DEFAULT_EPSS_BASE_URL)?;
    let kev_url = url_from_env(KEV_URL_ENV_VAR, DEFAULT_KEV_URL)?;
    validate_basilisk_url(&basilisk_url)?;

    let client = || -> miette::Result<reqwest::ClientBuilder> {
        Ok(pixi_utils::reqwest::reqwest_client_builder(Some(config))?
            .danger_accept_invalid_certs(config.tls_no_verify()))
    };
    let general = client()?.build().map_err(AuditCommandError::HttpClient)?;
    let basilisk_http = client()?
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(AuditCommandError::HttpClient)?;

    let storage = pixi_auth::get_auth_store(config).map_err(AuditCommandError::AuthStorage)?;
    let token = resolve_basilisk_token(
        std::env::var(TOKEN_ENV_VAR).ok(),
        Some(&storage),
        &basilisk_url,
    )?;
    if let Some((_, source)) = &token {
        tracing::info!(
            "auditing with the Basilisk API token from {}",
            match source {
                TokenSource::Environment => TOKEN_ENV_VAR,
                TokenSource::AuthStore => "the stored credential for the audit host",
            }
        );
    }

    let offline = config.offline();
    let basilisk = basilisk_middleware(
        base_stack(basilisk_http, offline),
        &basilisk_url,
        Some(storage),
        token.map(|(token, _)| token),
    )
    .build();
    let osv = base_stack(general.clone(), offline).build();
    let threat = threat_feed_client(
        base_stack(general, offline).build(),
        config.cache_dir_for(CacheKind::Audit)?.join("http-cache"),
        offline,
    );

    Ok(AuditClients {
        basilisk: BasiliskClient::new(basilisk, basilisk_url.clone()),
        osv: OsvDevClient::new(osv, osv_url.clone()),
        epss: EpssClient::new(threat.clone(), epss_url),
        kev: KevClient::new(threat, kev_url),
        basilisk_url,
        osv_url,
    })
}

/// Offline rejection first, so no request reaches the network, then retries.
fn base_stack(client: reqwest::Client, offline: bool) -> ClientBuilder {
    let mut builder = ClientBuilder::new(client);
    if offline {
        builder = builder.with(OfflineMiddleware);
    }
    builder.with(RetryTransientMiddleware::new_with_policy(
        pixi_utils::reqwest::default_retry_policy(),
    ))
}

/// Threat-feed responses are cached, so the KEV catalog is revalidated rather
/// than downloaded on every run. The cache sits in front of the caller's
/// stack: offline, a cached response is served regardless of freshness and
/// only a miss reaches the offline rejection. Cached feeds only enrich
/// rankings; they are not an offline vulnerability database.
fn threat_feed_client(
    stack: ClientWithMiddleware,
    cache_dir: PathBuf,
    offline: bool,
) -> ClientWithMiddleware {
    ClientBuilder::new(reqwest::Client::new())
        .with(Cache(HttpCache {
            mode: if offline {
                CacheMode::ForceCache
            } else {
                CacheMode::Default
            },
            manager: CACacheManager {
                path: cache_dir,
                remove_opts: Default::default(),
            },
            options: HttpCacheOptions::default(),
        }))
        .with(DelegateToClient(stack))
        .build()
}

/// Terminal middleware handing the request to another client's stack.
struct DelegateToClient(ClientWithMiddleware);

#[async_trait::async_trait]
impl Middleware for DelegateToClient {
    async fn handle(
        &self,
        request: Request,
        extensions: &mut http::Extensions,
        _next: Next<'_>,
    ) -> reqwest_middleware::Result<Response> {
        self.0.execute_with_extensions(request, extensions).await
    }
}

/// A Basilisk user API token bound to one origin, redacted in debug output.
#[derive(Clone)]
pub(super) struct BasiliskToken {
    token: String,
    origin: Origin,
}

impl BasiliskToken {
    fn parse(token: String, origin: Origin) -> miette::Result<Self> {
        if !looks_like_basilisk_token(&token) {
            miette::bail!(
                "invalid Basilisk API token (expected `bsk_` followed by 40 alphanumeric characters)"
            );
        }
        Ok(Self { token, origin })
    }
}

impl std::fmt::Debug for BasiliskToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BasiliskToken")
            .field("token", &"<redacted>")
            .field("origin", &self.origin.ascii_serialization())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TokenSource {
    Environment,
    AuthStore,
}

/// Tokens travel only over HTTPS, or plain HTTP to the loopback interface.
fn token_transport_allowed(url: &Url) -> bool {
    match url.scheme() {
        "https" => true,
        "http" => matches!(
            url.host(),
            Some(url::Host::Domain("localhost"))
                | Some(url::Host::Ipv4(std::net::Ipv4Addr::LOCALHOST))
                | Some(url::Host::Ipv6(std::net::Ipv6Addr::LOCALHOST))
        ),
        _ => false,
    }
}

fn validate_basilisk_url(url: &Url) -> miette::Result<()> {
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        miette::bail!(
            "{BASILISK_URL_ENV_VAR} must be an HTTP(S) URL without credentials, query parameters, or a fragment"
        );
    }
    Ok(())
}

/// Prefers the environment token, then a bearer token stored for the audit
/// host.
pub(super) fn resolve_basilisk_token(
    env_value: Option<String>,
    storage: Option<&AuthenticationStorage>,
    base_url: &Url,
) -> miette::Result<Option<(BasiliskToken, TokenSource)>> {
    let origin = base_url.origin();
    if let Some(value) = env_value {
        let value = value.trim().to_owned();
        if value.is_empty() {
            return Ok(None);
        }
        let token = BasiliskToken::parse(value, origin)?;
        if !token_transport_allowed(base_url) {
            miette::bail!(
                "{TOKEN_ENV_VAR} is set but the audit API URL is neither HTTPS nor local HTTP, so the token is not sent"
            );
        }
        return Ok(Some((token, TokenSource::Environment)));
    }
    let (Some(storage), true, Some(host)) = (
        storage,
        token_transport_allowed(base_url),
        base_url.host_str(),
    ) else {
        return Ok(None);
    };
    match storage.get(host) {
        Ok(Some(Authentication::BearerToken(value))) => Ok(BasiliskToken::parse(value, origin)
            .ok()
            .map(|token| (token, TokenSource::AuthStore))),
        _ => Ok(None),
    }
}

struct BasiliskTokenMiddleware(BasiliskToken);

#[async_trait::async_trait]
impl Middleware for BasiliskTokenMiddleware {
    async fn handle(
        &self,
        mut request: Request,
        extensions: &mut http::Extensions,
        next: Next<'_>,
    ) -> reqwest_middleware::Result<Response> {
        if request.url().origin() == self.0.origin && !request.headers().contains_key(AUTHORIZATION)
        {
            let mut value = HeaderValue::from_str(&format!("Bearer {}", self.0.token))
                .map_err(reqwest_middleware::Error::middleware)?;
            value.set_sensitive(true);
            request.headers_mut().insert(AUTHORIZATION, value);
        }
        next.run(request, extensions).await
    }
}

fn production_origin(url: &Url) -> bool {
    url.origin()
        == Url::parse(DEFAULT_BASILISK_URL)
            .expect("the default Basilisk URL is valid")
            .origin()
}

/// Adds the Basilisk credential to `builder`, without ever initiating a
/// login.
pub(super) fn basilisk_middleware(
    builder: ClientBuilder,
    base_url: &Url,
    storage: Option<AuthenticationStorage>,
    token: Option<BasiliskToken>,
) -> ClientBuilder {
    if let Some(token) = token
        && token_transport_allowed(base_url)
    {
        builder.with(BasiliskTokenMiddleware(token))
    } else if production_origin(base_url)
        && let Some(storage) = storage
    {
        builder.with(SharedLogin {
            authentication: AuthenticationMiddleware::from_auth_storage(storage.clone())
                .with_credentials_from(
                    url::Host::Domain(SHARED_LOGIN_KEY.to_owned()),
                    [base_url.origin()],
                ),
            storage,
            origin: base_url.origin(),
            warned: Arc::new(AtomicBool::new(false)),
        })
    } else {
        builder
    }
}

/// Borrows the saved `prefix.dev` login for the production Basilisk origin,
/// validating its audience before each request.
struct SharedLogin {
    authentication: AuthenticationMiddleware,
    storage: AuthenticationStorage,
    origin: Origin,
    warned: Arc<AtomicBool>,
}

impl SharedLogin {
    fn available(&self) -> bool {
        match self.storage.get(SHARED_LOGIN_KEY) {
            Ok(Some(Authentication::OAuth {
                audience,
                access_token,
                ..
            })) => {
                let usable = audience.as_deref() == Some(DEFAULT_BASILISK_URL)
                    && !access_token.is_empty()
                    && HeaderValue::from_str(&format!("Bearer {access_token}")).is_ok();
                if !usable && !self.warned.swap(true, Ordering::Relaxed) {
                    tracing::warn!(
                        "the saved prefix.dev login has no Basilisk audience or an invalid token, so the audit continues anonymously. Run `pixi auth login` to log in again"
                    );
                }
                usable
            }
            // Channel API tokens are not Basilisk credentials.
            Ok(_) => false,
            Err(_) => {
                if !self.warned.swap(true, Ordering::Relaxed) {
                    tracing::warn!(
                        "could not read the prefix.dev credentials, so the audit continues anonymously"
                    );
                }
                false
            }
        }
    }
}

#[async_trait::async_trait]
impl Middleware for SharedLogin {
    async fn handle(
        &self,
        request: Request,
        extensions: &mut http::Extensions,
        next: Next<'_>,
    ) -> reqwest_middleware::Result<Response> {
        if request.url().origin() != self.origin || !self.available() {
            return next.run(request, extensions).await;
        }
        self.authentication.handle(request, extensions, next).await
    }
}

#[cfg(test)]
mod tests;
