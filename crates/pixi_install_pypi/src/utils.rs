use std::{borrow::Cow, str::FromStr, time::Duration};

use url::Url;
use uv_cache_info::{CacheInfo, CacheInfoError};
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

/// Check freshness of a locked url against an installed dist.
///
/// For directory sources (source distributions), this function uses uv's `CacheInfo`
/// to determine if the source has changed since the package was installed. It compares
/// the current source's CacheInfo against the stored CacheInfo (from uv_cache.json)
/// that was captured at install time.
///
/// For file URLs pointing to files (wheel archives), freshness is determined by
/// comparing file timestamps since wheels are immutable artifacts.
///
/// This respects `[tool.uv].cache-keys` from pyproject.toml, falling back to uv's
/// defaults (pyproject.toml, setup.py, setup.cfg, and the src/ directory).
pub fn check_url_freshness(
    locked_url: &Url,
    installed_dist: &InstalledDist,
) -> Result<bool, CacheInfoError> {
    if let Ok(source_path) = locked_url.to_file_path() {
        // For files (wheels), use simple timestamp comparison
        if source_path.is_file() {
            // For wheel files, compare the file's modification time against METADATA
            let source_timestamp =
                uv_cache_info::Timestamp::from_path(&source_path).map_err(CacheInfoError::Io)?;
            let installed_timestamp =
                uv_cache_info::Timestamp::from_path(installed_dist.install_path().join("METADATA"))
                    .map_err(CacheInfoError::Io)?;

            let is_fresh = source_timestamp <= installed_timestamp;
            tracing::debug!(
                "archive freshness check for {installed_dist}: source_ts={source_timestamp:?}, installed_ts={installed_timestamp:?}, is_fresh={is_fresh}"
            );
            return Ok(is_fresh);
        }

        // For directories (source distributions), use CacheInfo comparison
        // Get current source cache info (reads [tool.uv.cache-keys] if present, else uses defaults)
        let source_cache_info = CacheInfo::from_path(&source_path)?;

        // Get the stored cache info from the installed distribution (uv_cache.json)
        let installed_cache_info = match InstalledDist::read_cache_info(
            installed_dist.install_path(),
        ) {
            Ok(Some(cache_info)) => cache_info,
            Ok(None) => {
                // No stored cache info (older installation or non-uv install)
                // Fall back to assuming not up-to-date to trigger a rebuild
                tracing::debug!(
                    "no stored cache info for {installed_dist}, assuming not up-to-date"
                );
                return Ok(false);
            }
            Err(err) => {
                tracing::debug!(
                    "failed to read stored cache info for {installed_dist}: {err}, assuming not up-to-date"
                );
                return Ok(false);
            }
        };

        // Compare CacheInfo objects - if they match, the source hasn't changed
        let is_fresh = source_cache_info == installed_cache_info;

        tracing::debug!(
            "freshness check for {installed_dist}: source={source_cache_info:?}, installed={installed_cache_info:?}, is_fresh={is_fresh}"
        );

        if is_fresh {
            tracing::debug!("requirement already satisfied (and up-to-date): {installed_dist}");
        } else {
            tracing::debug!("requirement already satisfied (but not up-to-date): {installed_dist}");
        }
        Ok(is_fresh)
    } else {
        // Non-local URLs assumed up-to-date
        tracing::debug!("requirement already satisfied (assumed up-to-date): {installed_dist}");
        Ok(true)
    }
}
