use std::sync::Mutex;

use rattler_networking::authentication_storage::backends::file::FileStorage;
use reqwest::StatusCode;

use super::*;

/// Records the request that reaches the network instead of sending it.
struct Capture(Arc<Mutex<Vec<Request>>>);

#[async_trait::async_trait]
impl Middleware for Capture {
    async fn handle(
        &self,
        request: Request,
        _extensions: &mut http::Extensions,
        _next: Next<'_>,
    ) -> reqwest_middleware::Result<Response> {
        self.0.lock().expect("not poisoned").push(request);
        Ok(http::Response::builder()
            .status(StatusCode::OK)
            .body("{}")
            .expect("valid response")
            .into())
    }
}

fn token_value() -> String {
    format!("bsk_{}", "A".repeat(40))
}

fn storage_with(entries: &[(&str, Authentication)]) -> (tempfile::TempDir, AuthenticationStorage) {
    let dir = tempfile::tempdir().expect("temporary directory");
    let path = dir.path().join("credentials.json");
    let mut storage = AuthenticationStorage::empty();
    storage.add_backend(Arc::new(
        FileStorage::from_path(path).expect("file storage"),
    ));
    for (host, authentication) in entries {
        storage
            .store(host, authentication)
            .expect("store credential");
    }
    (dir, storage)
}

fn shared_login(audience: Option<&str>) -> Authentication {
    Authentication::OAuth {
        audience: audience.map(str::to_string),
        access_token: "fixture-pixi-access-token".into(),
        refresh_token: None,
        expires_at: Some(i64::MAX),
        token_endpoint: "https://prefix.dev/token".into(),
        revocation_endpoint: None,
        client_id: "rattler".into(),
    }
}

/// The `Authorization` header each URL receives through a Basilisk client
/// for `base`.
async fn authorization_sent(
    base: &str,
    storage: Option<AuthenticationStorage>,
    token: Option<BasiliskToken>,
    urls: &[&str],
) -> Vec<Option<String>> {
    let base = Url::parse(base).expect("valid URL");
    let captured = Arc::new(Mutex::new(Vec::new()));
    let client = basilisk_middleware(
        ClientBuilder::new(reqwest::Client::new()),
        &base,
        storage,
        token,
    )
    .with(Capture(captured.clone()))
    .build();
    for url in urls {
        client.post(*url).send().await.expect("captured response");
    }
    let requests = captured.lock().expect("not poisoned");
    requests
        .iter()
        .map(|request| {
            request
                .headers()
                .get(AUTHORIZATION)
                .map(|value| value.to_str().expect("ascii header").to_string())
        })
        .collect()
}

#[tokio::test]
async fn api_token_is_sent_only_to_the_exact_origin() {
    let base = "https://audit.example";
    let (token, source) =
        resolve_basilisk_token(Some(token_value()), None, &Url::parse(base).expect("url"))
            .expect("valid token")
            .expect("token");
    assert_eq!(source, TokenSource::Environment);
    let sent = authorization_sent(
        base,
        None,
        Some(token),
        &[
            "https://audit.example/v1/querybatch",
            "https://audit.example:8443/v1/querybatch",
            "http://audit.example/v1/querybatch",
            "https://audit.example.evil.example/v1/querybatch",
            "https://api.osv.dev/v1/querybatch",
        ],
    )
    .await;
    assert_eq!(
        sent,
        vec![
            Some(format!("Bearer {}", token_value())),
            None,
            None,
            None,
            None
        ]
    );
}

#[tokio::test]
async fn saved_login_is_borrowed_only_for_production_with_a_basilisk_audience() {
    let (_dir, storage) =
        storage_with(&[(SHARED_LOGIN_KEY, shared_login(Some(DEFAULT_BASILISK_URL)))]);
    let sent = authorization_sent(
        DEFAULT_BASILISK_URL,
        Some(storage.clone()),
        None,
        &[
            "https://api.basilisk.prefix.dev/v1/querybatch",
            "https://prefix.dev/api",
        ],
    )
    .await;
    assert_eq!(
        sent,
        vec![Some("Bearer fixture-pixi-access-token".to_string()), None]
    );

    // A custom deployment never borrows the shared login.
    let sent = authorization_sent(
        "https://audit.example",
        Some(storage),
        None,
        &["https://audit.example/v1/querybatch"],
    )
    .await;
    assert_eq!(sent, vec![None]);

    for audience in [None, Some("https://prefix.dev")] {
        let (_dir, storage) = storage_with(&[(SHARED_LOGIN_KEY, shared_login(audience))]);
        let sent = authorization_sent(
            DEFAULT_BASILISK_URL,
            Some(storage),
            None,
            &["https://api.basilisk.prefix.dev/v1/querybatch"],
        )
        .await;
        assert_eq!(sent, vec![None], "audience {audience:?}");
    }
}

#[test]
fn stored_tokens_are_looked_up_for_the_audit_host_over_a_safe_transport() {
    let (_dir, storage) = storage_with(&[
        ("audit.example", Authentication::BearerToken(token_value())),
        ("localhost", Authentication::BearerToken(token_value())),
        ("plain.example", Authentication::BearerToken(token_value())),
    ]);
    let resolve = |url: &str| {
        resolve_basilisk_token(None, Some(&storage), &Url::parse(url).expect("url"))
            .expect("resolves")
            .map(|(_, source)| source)
    };
    assert_eq!(
        resolve("https://audit.example"),
        Some(TokenSource::AuthStore)
    );
    assert_eq!(
        resolve("http://localhost:8080"),
        Some(TokenSource::AuthStore)
    );
    assert_eq!(resolve("http://plain.example"), None);
    assert_eq!(resolve("https://other.example"), None);
}

#[test]
fn environment_tokens_are_validated_and_never_sent_over_plain_http() {
    let url = Url::parse("http://audit.example").expect("url");
    let err = resolve_basilisk_token(Some(token_value()), None, &url).unwrap_err();
    assert_eq!(
        err.to_string(),
        "PIXI_AUDIT_TOKEN is set but the audit API URL is neither HTTPS nor local HTTP, so the token is not sent"
    );
    let url = Url::parse("https://audit.example").expect("url");
    let err = resolve_basilisk_token(Some("not-a-token".to_string()), None, &url).unwrap_err();
    assert_eq!(
        err.to_string(),
        "invalid Basilisk API token (expected `bsk_` followed by 40 alphanumeric characters)"
    );
    assert!(
        resolve_basilisk_token(Some("  ".to_string()), None, &url)
            .expect("empty is unset")
            .is_none()
    );
}

#[test]
fn basilisk_urls_carry_no_credentials_or_query() {
    for url in [
        "https://user:secret@audit.example",
        "https://audit.example/?token=1",
        "https://audit.example/#fragment",
        "ftp://audit.example",
    ] {
        assert!(
            validate_basilisk_url(&Url::parse(url).expect("url")).is_err(),
            "{url}"
        );
    }
    assert!(
        validate_basilisk_url(&Url::parse("https://audit.example/prefix").expect("url")).is_ok()
    );
}
