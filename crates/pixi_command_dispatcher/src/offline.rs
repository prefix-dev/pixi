//! Deciding which candidates an offline solve may pick.
//!
//! In offline mode a solve may only select packages that can be installed
//! without network access. Rather than dropping the other records before
//! solving, they are handed to the solver as *excluded* candidates together
//! with the reason, so that a solve which fails because of the restriction can
//! say so instead of reporting that a package the repodata plainly lists has
//! no candidates.

use std::{collections::HashMap, sync::Arc};

use rattler_cache::package_cache::{CacheIndex, PackageCache};
use rattler_conda_types::RepoDataRecord;
use url::Url;

/// The reason recorded for candidates ruled out by offline mode.
///
/// This is user-facing: it is rendered by the solver as part of the conflict
/// report when an exclusion is what made a solve impossible.
pub const OFFLINE_EXCLUSION_REASON: &str = "not available locally";

/// Whether a record can be installed without touching the network.
///
/// Two cases qualify. A record served from a local channel needs no download
/// at all, whatever the package cache happens to hold. Any other record has to
/// already be in the package cache.
///
/// Note that pixi's package cache does not key entries by origin, so a package
/// cached from one channel counts as available for the same name, version and
/// build coming from another. That matches how the installer would resolve it
/// anyway.
pub fn is_locally_available(index: &CacheIndex, record: &RepoDataRecord) -> bool {
    record.url.scheme() == "file" || index.contains_record(record)
}

/// Builds the exclusion map for an offline solve.
///
/// The map is keyed by record URL, matching
/// `rattler_solve::SolverTask::excluded_candidates`.
pub fn offline_exclusions<'a>(
    index: &CacheIndex,
    records: impl IntoIterator<Item = &'a RepoDataRecord>,
) -> HashMap<Url, Arc<str>> {
    // Every exclusion carries the same reason, and a solve can rule out tens
    // of thousands of records, so the string is allocated once and shared.
    let reason: Arc<str> = Arc::from(OFFLINE_EXCLUSION_REASON);
    records
        .into_iter()
        .filter(|record| !is_locally_available(index, record))
        .map(|record| (record.url.clone(), Arc::clone(&reason)))
        .collect()
}

/// Builds the exclusion map for a solve that may or may not be restricted.
///
/// This is the form every solve site wants: when `offline` is off the answer
/// is an empty map, and no cache is read at all. Going through one function
/// keeps "unrestricted" and "nothing to exclude" spelled the same way
/// everywhere, which is what the solver reads them as.
///
/// The cache is indexed per solve rather than once per run, so packages
/// downloaded by an earlier solve in the same run count as available.
pub async fn exclusions_for_solve<'a>(
    offline: bool,
    package_cache: &PackageCache,
    records: impl IntoIterator<Item = &'a RepoDataRecord>,
) -> std::io::Result<HashMap<Url, Arc<str>>> {
    if !offline {
        return Ok(HashMap::new());
    }

    let index = package_cache.index().await?;
    Ok(offline_exclusions(&index, records))
}

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        str::FromStr,
    };

    use rattler_cache::package_cache::PackageCache;
    use rattler_conda_types::{
        PackageName, PackageRecord, RepoDataRecord, Version, VersionWithSource,
        package::{ArchiveIdentifier, CondaArchiveType, DistArchiveIdentifier, DistArchiveType},
    };
    use tempfile::TempDir;

    use super::*;

    /// A real package archive, so the cache is populated the way it would be
    /// in production rather than by hand-building directory names.
    fn test_archive() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/data/installation-order/foobar/foobar-0.1.0-pyhbf21a9e_0.conda")
    }

    fn record(name: &str, version: &str, build: &str, url: &str) -> RepoDataRecord {
        RepoDataRecord {
            url: Url::parse(url).unwrap(),
            channel: None,
            identifier: DistArchiveIdentifier {
                identifier: ArchiveIdentifier {
                    name: name.to_string(),
                    version: version.to_string(),
                    build_string: build.to_string(),
                },
                archive_type: DistArchiveType::Conda(CondaArchiveType::Conda),
            },
            package_record: PackageRecord::new(
                PackageName::new_unchecked(name),
                VersionWithSource::from(Version::from_str(version).unwrap()),
                build.to_string(),
            ),
        }
    }

    /// A record served from a local channel needs no download, so it counts as
    /// available even though nothing has ever been cached.
    #[tokio::test]
    async fn file_url_records_are_locally_available() {
        let cache_dir = TempDir::new().unwrap();
        let index = PackageCache::new(cache_dir.path()).index().await.unwrap();

        let record = record(
            "foo",
            "1.0",
            "h1",
            "file:///channel/noarch/foo-1.0-h1.conda",
        );

        assert!(is_locally_available(&index, &record));
        assert!(offline_exclusions(&index, [&record]).is_empty());
    }

    /// Anything that would have to be downloaded is excluded, and carries the
    /// reason the solver reports back.
    #[tokio::test]
    async fn uncached_remote_records_are_excluded_with_a_reason() {
        let cache_dir = TempDir::new().unwrap();
        let index = PackageCache::new(cache_dir.path()).index().await.unwrap();

        let record = record(
            "foo",
            "1.0",
            "h1",
            "https://example.com/noarch/foo-1.0-h1.conda",
        );

        assert!(!is_locally_available(&index, &record));
        assert_eq!(
            offline_exclusions(&index, [&record]).get(&record.url),
            Some(&Arc::from(OFFLINE_EXCLUSION_REASON))
        );
    }

    /// Once the package really is in the cache the same remote record counts
    /// as available. The cache is populated through the normal API so this
    /// does not encode the on-disk layout itself.
    #[tokio::test]
    async fn cached_remote_records_are_locally_available() {
        let cache_dir = TempDir::new().unwrap();
        let cache = PackageCache::new(cache_dir.path());
        let archive = test_archive();

        cache
            .get_or_fetch_from_path(&archive, None, None)
            .await
            .unwrap();

        let index = cache.index().await.unwrap();
        let record = record(
            "foobar",
            "0.1.0",
            "pyhbf21a9e_0",
            "https://example.com/noarch/foobar-0.1.0-pyhbf21a9e_0.conda",
        );

        assert!(
            is_locally_available(&index, &record),
            "a package in the cache should count as locally available"
        );
        assert!(offline_exclusions(&index, [&record]).is_empty());
    }
}
