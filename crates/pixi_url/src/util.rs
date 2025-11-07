use std::hash::{DefaultHasher, Hash, Hasher};

use url::Url;

/// Computes a deterministic identifier for the provided URL.
pub fn cache_digest(url: &Url) -> String {
    let mut hasher = DefaultHasher::new();
    url.as_str().hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

/// Attempts to derive a filename from the URL's last path segment.
pub fn url_file_name(url: &Url) -> String {
    url.path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|segment| !segment.is_empty())
        .map(|segment| segment.to_string())
        .unwrap_or_else(|| "download".to_string())
}
