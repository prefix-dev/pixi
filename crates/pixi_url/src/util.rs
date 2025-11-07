use rattler_digest::{Sha256, digest::Digest};
use url::Url;

/// Computes a deterministic identifier for the provided URL.
pub fn cache_digest(url: &Url) -> String {
    let digest = Sha256::digest(url.as_str().as_bytes());
    format!("{digest:x}")
}

/// Attempts to derive a filename from the URL's last path segment.
pub fn url_file_name(url: &Url) -> String {
    url.path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|segment| !segment.is_empty())
        .map(|segment| segment.to_string())
        .unwrap_or_else(|| "download".to_string())
}
