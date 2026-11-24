use std::str::FromStr;

use rattler_conda_types::RepoDataRecord;
use url::Url;

/// Returns `true` if the specified record refers to a conda-forge package.
pub fn is_conda_forge_record(record: &RepoDataRecord) -> bool {
    record_channel_url(record).is_some_and(|url| is_conda_forge_url(&url))
}

/// Returns `true` if the specified url refers to a conda-forge channel.
pub fn is_conda_forge_url(url: &Url) -> bool {
    is_channel_url(url, "conda-forge")
}

/// Returns `true` if the specified record comes from the conda-pypi channel.
///
/// The conda-pypi channel serves PyPI wheels as conda packages, so the conda
/// package name of such a record is the PyPI package name by construction.
pub fn is_conda_pypi_record(record: &RepoDataRecord) -> bool {
    record_channel_url(record).is_some_and(|url| is_conda_pypi_url(&url))
}

/// Returns `true` if the specified url refers to the conda-pypi channel.
pub fn is_conda_pypi_url(url: &Url) -> bool {
    is_channel_url(url, "conda-pypi")
}

/// Returns `true` if the path of `url` starts with the channel `name`, which
/// matches the channel on any host, including mirrors.
fn is_channel_url(url: &Url, name: &str) -> bool {
    url.path()
        .strip_prefix('/')
        .is_some_and(|path| path.starts_with(name))
}

/// Parses the channel of a record as a URL, if it has one.
fn record_channel_url(record: &RepoDataRecord) -> Option<Url> {
    record
        .channel
        .as_ref()
        .and_then(|channel| Url::from_str(channel).ok())
}

/// Normalize channel strings so project-defined mappings and repodata records can be compared.
pub(crate) fn normalize_channel(channel: &str) -> &str {
    channel.trim_end_matches('/')
}
