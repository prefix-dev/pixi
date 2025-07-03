use std::{borrow::Cow, cmp::max, io, path::Path, str::FromStr, time::Duration};

use url::Url;
use uv_cache_info::Timestamp;
use uv_distribution_types::InstalledDist;

pub fn elapsed(duration: Duration) -> String {
    let secs = duration.as_secs();

    if secs >= 60 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else if secs > 0 {
        format!("{}.{:02}s", secs, duration.subsec_nanos() / 10_000_000)
    } else {
        format!("{}ms", duration.subsec_millis())
    }
}

/// Check if the url is a direct url
/// Files, git, are direct urls
/// Direct urls to wheels or sdists are prefixed with a `direct` scheme
/// by us when resolving the lock file
pub fn is_direct_url(url_scheme: &str) -> bool {
    url_scheme == "file"
        || url_scheme == "git+http"
        || url_scheme == "git+https"
        || url_scheme == "git+ssh"
        || url_scheme == "git+file"
        || url_scheme.starts_with("direct")
}

/// Strip of the `direct` scheme from the url if it is there
pub fn strip_direct_scheme(url: &Url) -> Cow<'_, Url> {
    url.as_ref()
        .strip_prefix("direct+")
        .and_then(|str| Url::from_str(str).ok())
        .map(Cow::Owned)
        .unwrap_or(Cow::Borrowed(url))
}

/// ArchiveTimestamp is removed from uv,
/// copy logic from ArchiveTimestamp::from_path to keep the same behavior
fn uv_old_timestamp_from_path(path: impl AsRef<Path>) -> Result<Option<Timestamp>, io::Error> {
    let metadata = fs_err::metadata(path.as_ref())?;
    if metadata.is_file() {
        Ok(Some(Timestamp::from_metadata(&metadata)))
    } else {
        // Compute the modification timestamp for the `pyproject.toml`, `setup.py`, and
        // `setup.cfg` files, if they exist.
        let pyproject_toml = path
            .as_ref()
            .join("pyproject.toml")
            .metadata()
            .ok()
            .filter(std::fs::Metadata::is_file)
            .as_ref()
            .map(Timestamp::from_metadata);

        let setup_py = path
            .as_ref()
            .join("setup.py")
            .metadata()
            .ok()
            .filter(std::fs::Metadata::is_file)
            .as_ref()
            .map(Timestamp::from_metadata);

        let setup_cfg = path
            .as_ref()
            .join("setup.cfg")
            .metadata()
            .ok()
            .filter(std::fs::Metadata::is_file)
            .as_ref()
            .map(Timestamp::from_metadata);

        // Take the most recent timestamp of the three files.
        Ok(max(pyproject_toml, max(setup_py, setup_cfg)))
    }
}

/// ArchiveTimestamp is removed from uv,
/// copy logic from ArchiveTimestamp::up_to_date_with to keep the same behavior
fn uv_old_up_to_date_with(source: &Path, target: &InstalledDist) -> Result<bool, io::Error> {
    let Some(modified_at) = uv_old_timestamp_from_path(source)? else {
        // If there's no entrypoint, we can't determine the modification time, so we assume that the
        // target is not up-to-date.
        return Ok(false);
    };
    let created_at = Timestamp::from_path(target.install_path().join("METADATA"))?;
    Ok(modified_at <= created_at)
}

/// Check freshness of a locked url against an installed dist
pub fn check_url_freshness(
    locked_url: &Url,
    installed_dist: &InstalledDist,
) -> Result<bool, std::io::Error> {
    if let Ok(archive) = locked_url.to_file_path() {
        // This checks the entrypoints like `pyproject.toml`, `setup.cfg`, and
        // `setup.py` against the METADATA of the installed distribution
        if uv_old_up_to_date_with(&archive, installed_dist)? {
            tracing::debug!("Requirement already satisfied (and up-to-date): {installed_dist}");
            Ok(true)
        } else {
            tracing::debug!("Requirement already satisfied (but not up-to-date): {installed_dist}");
            Ok(false)
        }
    } else {
        // Otherwise, assume the requirement is up-to-date.
        tracing::debug!("Requirement already satisfied (assumed up-to-date): {installed_dist}");
        Ok(true)
    }
}
