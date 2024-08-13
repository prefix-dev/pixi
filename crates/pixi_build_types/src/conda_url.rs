use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged, rename_all = "camelCase")]
/// Different types of URLs that can be used to fetch conda packages.
pub enum CondaUrl {
    /// A URL that uses the HTTP protocol.
    Http(Url),
    /// A URL that uses the HTTPS protocol.
    Https(Url),
    /// A URL that uses the file protocol.
    File(Url),
}
