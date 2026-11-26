//! The request path every client shares: send, check the status, decode
//! the JSON body. Callers attach the URL and name the service when they
//! turn an [`HttpFailure`] into their own error type.

use reqwest_middleware::RequestBuilder;
use serde::de::DeserializeOwned;
use url::Url;

/// Why an HTTP exchange failed.
#[derive(Debug)]
pub enum HttpFailure {
    Request(reqwest_middleware::Error),
    Status {
        status: reqwest::StatusCode,
        /// The server's `{"error": "..."}` body, when present.
        message: Option<String>,
    },
    InvalidResponse(reqwest::Error),
}

/// `url` with a trailing slash on its path. `Url::join` treats a path
/// without one as a file: `https://host/prefix` + `v1/querybatch` would
/// drop `prefix`.
pub fn base_url(mut url: Url) -> Url {
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    url
}

/// Sends `request` and decodes a successful JSON response.
pub async fn send_json<T: DeserializeOwned>(request: RequestBuilder) -> Result<T, HttpFailure> {
    let response = request.send().await.map_err(HttpFailure::Request)?;
    let status = response.status();
    if !status.is_success() {
        let message = response
            .text()
            .await
            .ok()
            .and_then(|body| error_message(&body));
        return Err(HttpFailure::Status { status, message });
    }
    response.json().await.map_err(HttpFailure::InvalidResponse)
}

/// The `error` field of a `{"error": "..."}` body (basilisk's error format).
fn error_message(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("error")?
        .as_str()
        .map(str::to_string)
}
