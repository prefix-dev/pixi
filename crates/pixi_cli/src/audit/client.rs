//! Audit transport is separate from package downloads: no channel credentials,
//! mirror rewriting, or automatic challenge-driven login.

use std::{io::IsTerminal, time::Duration};

use miette::IntoDiagnostic;
use pixi_auth::get_auth_store;
use pixi_config::Config;
use pixi_consts::consts;
use pixi_utils::reqwest::{default_retry_policy, reqwest_client_builder};
use rattler::cli::auth::oauth::{
    CallbackPageTemplate, OAuthConfig, OAuthFlow, callback_page_renderer, perform_oauth_login,
};
use rattler_networking::{AuthenticationMiddleware, AuthenticationStorage, OfflineMiddleware};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::RetryTransientMiddleware;
use url::Url;

use pixi_audit::DEFAULT_BASE_URL;

const ISSUER: &str = "https://prefix.dev";
const CLIENT_ID: &str = "rattler";

fn production_origin(base_url: &Url) -> bool {
    base_url.origin()
        == Url::parse(DEFAULT_BASE_URL)
            .expect("valid default API URL")
            .origin()
}

fn validate_base_url(base_url: &Url) -> miette::Result<()> {
    if !matches!(base_url.scheme(), "http" | "https")
        || base_url.host_str().is_none()
        || !base_url.username().is_empty()
        || base_url.password().is_some()
        || base_url.query().is_some()
        || base_url.fragment().is_some()
    {
        miette::bail!(
            "Audit API URL must be HTTP(S), without credentials, query parameters, or a fragment"
        );
    }
    Ok(())
}

fn interactive_login_allowed(offline: bool, in_ci: bool, terminal: bool) -> bool {
    !offline && !in_ci && terminal
}

/// Build the audit client, optionally performing an explicitly requested login.
/// Credential failures never prevent an otherwise public audit.
pub(super) async fn build(
    config: &Config,
    base_url: &Url,
    login: bool,
) -> miette::Result<ClientWithMiddleware> {
    validate_base_url(base_url)?;
    let storage = if production_origin(base_url) && !config.offline() && !config.tls_no_verify() {
        match get_auth_store(config) {
            Ok(storage) => Some(storage),
            Err(_error) => {
                eprintln!("Warning: Could not load audit credentials; continuing anonymously");
                None
            }
        }
    } else {
        None
    };

    if login {
        if !production_origin(base_url) {
            eprintln!(
                "Audit login is only supported for the default Basilisk origin; continuing anonymously"
            );
        } else if config.tls_no_verify() {
            eprintln!(
                "Warning: Skipping audit login while TLS verification is disabled; continuing anonymously"
            );
        } else if !interactive_login_allowed(
            config.offline(),
            std::env::var_os("CI").is_some(),
            std::io::stdin().is_terminal() && std::io::stderr().is_terminal(),
        ) {
            eprintln!("Warning: Skipping audit login in offline, CI, or noninteractive mode");
        } else if let Some(storage) = &storage {
            let oauth = OAuthConfig {
                audience: Some(DEFAULT_BASE_URL.into()),
                issuer_url: ISSUER.into(),
                client_id: CLIENT_ID.into(),
                client_secret: None,
                flow: OAuthFlow::Auto,
                scopes: Default::default(),
                redirect_uri: None,
                user_agent: Some(format!("pixi/{}", consts::PIXI_VERSION)),
                callback_page: Some(callback_page_renderer(
                    CallbackPageTemplate {
                        application_name: "pixi".into(),
                        ..Default::default()
                    },
                    ISSUER,
                )),
            };
            match tokio::time::timeout(Duration::from_secs(300), perform_oauth_login(oauth)).await {
                Ok(Ok(auth)) => {
                    let key = AuthenticationStorage::oauth_audience_key(
                        ISSUER,
                        CLIENT_ID,
                        DEFAULT_BASE_URL,
                    );
                    if storage.store(&key, &auth).is_err() {
                        eprintln!(
                            "Could not save audit credentials; continuing with cached credentials if available"
                        );
                    }
                }
                Ok(Err(_)) | Err(_) => {
                    eprintln!(
                        "Audit login did not complete; continuing with cached credentials if available"
                    );
                }
            }
        }
    }

    Ok(client_builder(config, base_url, storage)?.build())
}

fn client_builder(
    config: &Config,
    base_url: &Url,
    storage: Option<AuthenticationStorage>,
) -> miette::Result<ClientBuilder> {
    validate_base_url(base_url)?;
    // Preserve Pixi's proxy/TLS configuration, but never send credentials over
    // unverified TLS and never redirect a bearer.
    let http = reqwest_client_builder(Some(config))?
        .danger_accept_invalid_certs(config.tls_no_verify())
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .into_diagnostic()?;
    let mut builder = ClientBuilder::new(http);
    if config.offline() {
        builder = builder.with(OfflineMiddleware);
    }
    builder = builder.with(RetryTransientMiddleware::new_with_policy(
        default_retry_policy(),
    ));
    if production_origin(base_url)
        && !config.offline()
        && !config.tls_no_verify()
        && let Some(storage) = storage
    {
        builder = builder.with(
            AuthenticationMiddleware::from_auth_storage(storage).with_oauth_audience(
                ISSUER,
                CLIENT_ID,
                DEFAULT_BASE_URL,
                base_url.origin(),
            ),
        );
    }
    Ok(builder)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::Form, http::StatusCode, routing::post};
    use rattler_networking::{
        Authentication, authentication_storage::backends::memory::MemoryStorage,
    };
    use serde_json::json;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    type Seen = Arc<Mutex<Vec<Option<String>>>>;
    struct Capture(Seen);
    #[async_trait::async_trait]
    impl reqwest_middleware::Middleware for Capture {
        async fn handle(
            &self,
            req: reqwest::Request,
            _extensions: &mut http::Extensions,
            _next: reqwest_middleware::Next<'_>,
        ) -> reqwest_middleware::Result<reqwest::Response> {
            self.0.lock().unwrap().push(
                req.headers()
                    .get("authorization")
                    .map(|v| v.to_str().unwrap().to_owned()),
            );
            Ok(http::Response::builder()
                .status(200)
                .body("{}")
                .unwrap()
                .into())
        }
    }
    fn key() -> String {
        AuthenticationStorage::oauth_audience_key(ISSUER, CLIENT_ID, DEFAULT_BASE_URL)
    }
    fn storage() -> AuthenticationStorage {
        let mut store = AuthenticationStorage::empty();
        store.add_backend(Arc::new(MemoryStorage::new()));
        store
    }
    fn grant(endpoint: String, expiry: i64) -> Authentication {
        Authentication::OAuth {
            audience: Some(DEFAULT_BASE_URL.into()),
            access_token: "fixture-audience".into(),
            refresh_token: Some("fixture-refresh".into()),
            expires_at: Some(expiry),
            token_endpoint: endpoint,
            revocation_endpoint: None,
            client_id: CLIENT_ID.into(),
        }
    }
    fn capture_client(
        config: &Config,
        url: &Url,
        store: Option<AuthenticationStorage>,
    ) -> (ClientWithMiddleware, Seen) {
        let seen = Seen::default();
        let http = client_builder(config, url, store)
            .unwrap()
            .with(Capture(seen.clone()))
            .build();
        (http, seen)
    }

    #[test]
    fn login_requires_online_interactive_non_ci_execution() {
        for offline in [false, true] {
            for ci in [false, true] {
                for terminal in [false, true] {
                    assert_eq!(
                        interactive_login_allowed(offline, ci, terminal),
                        !offline && !ci && terminal
                    );
                }
            }
        }
    }

    #[test]
    fn rejects_secret_bearing_and_non_http_base_urls() {
        for value in [
            "https://user:fixture-secret@api.example",
            "https://api.example/?token=fixture-secret",
            "https://api.example/#fixture-secret",
            "file:///tmp/api",
        ] {
            let error = validate_base_url(&Url::parse(value).unwrap()).unwrap_err();
            assert!(!error.to_string().contains("fixture-secret"));
        }
        assert!(production_origin(
            &Url::parse("https://API.BASILISK.PREFIX.DEV:443/").unwrap()
        ));
        assert!(!production_origin(
            &Url::parse("http://api.basilisk.prefix.dev").unwrap()
        ));
        assert!(!production_origin(
            &Url::parse("https://api.basilisk.prefix.dev:444").unwrap()
        ));
    }

    #[tokio::test]
    async fn public_audit_does_not_use_channel_credentials() {
        let url = Url::parse(DEFAULT_BASE_URL).unwrap();
        let store = storage();
        store
            .store(
                "api.basilisk.prefix.dev",
                &Authentication::BearerToken("fixture-channel".into()),
            )
            .unwrap();
        for store in [None, Some(store)] {
            let (http, seen) = capture_client(&Config::default(), &url, store);
            assert_eq!(
                http.post(url.clone()).send().await.unwrap().status(),
                StatusCode::OK
            );
            assert_eq!(*seen.lock().unwrap(), vec![None]);
        }
    }

    #[tokio::test]
    async fn cached_audience_is_sent_only_to_production_origin() {
        let store = storage();
        let auth = grant("http://127.0.0.1:1/token".into(), i64::MAX);
        store.store(&key(), &auth).unwrap();
        let production = Url::parse(DEFAULT_BASE_URL).unwrap();
        let (http, seen) = capture_client(&Config::default(), &production, Some(store.clone()));
        http.post(production.clone()).send().await.unwrap();
        http.post(production.clone()).send().await.unwrap();
        http.post("https://custom.example/api")
            .send()
            .await
            .unwrap();
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                Some("Bearer fixture-audience".into()),
                Some("Bearer fixture-audience".into()),
                None
            ]
        );
        assert_eq!(store.get(&key()).unwrap(), Some(auth));
        let insecure = Config {
            tls_no_verify: Some(true),
            ..Default::default()
        };
        let (http, seen) = capture_client(&insecure, &production, Some(store.clone()));
        http.post(production.clone()).send().await.unwrap();
        assert_eq!(*seen.lock().unwrap(), vec![None]);
        for url in [
            "https://custom.example/",
            "http://api.basilisk.prefix.dev/",
            "https://api.basilisk.prefix.dev:444/",
        ] {
            let url = Url::parse(url).unwrap();
            let (http, seen) = capture_client(&Config::default(), &url, Some(store.clone()));
            http.post(url).send().await.unwrap();
            assert_eq!(*seen.lock().unwrap(), vec![None]);
        }
    }

    #[tokio::test]
    async fn expired_audience_refreshes_or_falls_back_to_public_audit() {
        for success in [true, false] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}/token", listener.local_addr().unwrap());
            let calls = Arc::new(AtomicUsize::new(0));
            let count = calls.clone();
            let app = Router::new().route("/token", post(move |Form(form): Form<std::collections::HashMap<String,String>>| {
                assert_eq!(form["audience"], DEFAULT_BASE_URL);
                assert_eq!(form["client_id"], CLIENT_ID);
                assert_eq!(form["refresh_token"], "fixture-refresh");
                count.fetch_add(1, Ordering::SeqCst);
                async move {
                    if success { (StatusCode::OK, Json(json!({"access_token":"fixture-new", "refresh_token":"fixture-rotated", "expires_in":3600,"token_type":"Bearer"}))) }
                    else { (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error":"temporarily_unavailable"}))) }
                }
            }));
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let store = storage();
            let old = grant(endpoint, 0);
            store.store(&key(), &old).unwrap();
            let url = Url::parse(DEFAULT_BASE_URL).unwrap();
            let (http, seen) = capture_client(&Config::default(), &url, Some(store.clone()));
            assert_eq!(
                http.post(url).send().await.unwrap().status(),
                StatusCode::OK
            );
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            let expected = success.then(|| "Bearer fixture-new".to_owned());
            assert_eq!(*seen.lock().unwrap(), vec![expected]);
            if success {
                assert!(
                    matches!(store.get(&key()).unwrap(), Some(Authentication::OAuth { refresh_token: Some(refresh), .. }) if refresh == "fixture-rotated")
                );
            } else {
                assert_eq!(store.get(&key()).unwrap(), Some(old));
            }
            server.abort();
        }
    }

    #[tokio::test]
    async fn offline_blocks_before_authentication_and_transport() {
        let config = Config {
            offline: Some(true),
            ..Default::default()
        };
        let store = storage();
        store
            .store(&key(), &grant("http://127.0.0.1:1/token".into(), 0))
            .unwrap();
        let url = Url::parse(DEFAULT_BASE_URL).unwrap();
        let (http, seen) = capture_client(&config, &url, Some(store));
        assert!(http.post(url).send().await.is_err());
        assert!(seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn api_client_does_not_follow_redirects() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let app = Router::new()
            .route(
                "/redirect",
                post(|| async {
                    (
                        StatusCode::TEMPORARY_REDIRECT,
                        [("location", "/destination")],
                    )
                }),
            )
            .route(
                "/destination",
                post(move || {
                    count.fetch_add(1, Ordering::SeqCst);
                    async { "unexpected" }
                }),
            );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let http = client_builder(&Config::default(), &url, None)
            .unwrap()
            .build();
        assert_eq!(
            http.post(url.join("redirect").unwrap())
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::TEMPORARY_REDIRECT
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        server.abort();
    }
}
