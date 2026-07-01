use std::str::FromStr;

use rattler_conda_types::RepoDataRecord;
use url::Url;

/// Returns `true` if the specified record refers to a conda-forge package.
pub fn is_conda_forge_record(record: &RepoDataRecord) -> bool {
    record
        .channel
        .as_ref()
        .and_then(|channel| Url::from_str(channel).ok())
        .is_some_and(|u| is_conda_forge_url(&u))
}

/// Returns `true` if the specified url refers to a conda-forge channel.
pub fn is_conda_forge_url(url: &Url) -> bool {
    url.path().starts_with("/conda-forge")
}

/// Normalize channel strings so project-defined mappings and repodata records can be compared.
pub(crate) fn normalize_channel(channel: &str) -> &str {
    channel.trim_end_matches('/')
}
