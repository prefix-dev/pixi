use async_once_cell::OnceCell as AsyncCell;
use miette::{IntoDiagnostic, WrapErr};
use rattler_conda_types::RepoDataRecord;
use rattler_networking::LazyClient;
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use url::Url;

use crate::{
    CacheMetrics, CompressedMapping, MappingByChannel, MappingError, MappingMap, MappingMode,
    ProjectDefinedMappingLocation, PurlDerivationSource, ResolvedChannelMapping,
    channel::normalize_channel, derivation::DerivationOutcome, purl::pypi_purl,
};

/// Subdirectory of the conda-pypi mapping cache that holds TTL-cached
/// project-defined mappings.
const TTL_CACHE_SUBDIR: &str = "project-defined";

/// Struct with a mapping of channel names to their respective mapping
/// configuration: one or more sources (remote url, local file or in-memory
/// entries) and the mode that determines how the mapping interacts with the
/// default prefix.dev chain.
///
/// This struct caches the fetched mapping internally.
#[derive(Debug)]
pub struct ProjectDefinedMapping {
    pub mapping: MappingMap,
    mapping_value: AsyncCell<MappingByChannel>,
}

impl ProjectDefinedMapping {
    /// Create a new `ProjectDefinedMapping` with the specified mapping.
    pub fn new(mapping: MappingMap) -> Self {
        Self {
            mapping,
            mapping_value: Default::default(),
        }
    }

    /// Fetch the project-defined mapping from the server or load from the
    /// local filesystem. Each channel's sources are merged in order: entries
    /// from later sources override entries from earlier ones.
    pub async fn fetch_project_defined_mapping(
        &self,
        client: &LazyClient,
        cache_dir: &Path,
    ) -> miette::Result<MappingByChannel> {
        self.mapping_value
            .get_or_try_init(async {
                let mut mapping_url_to_name: MappingByChannel = Default::default();

                for (name, channel_mapping) in self.mapping.iter() {
                    let mut merged = CompressedMapping::default();
                    for source in &channel_mapping.sources {
                        let mapping_by_name = match source {
                            ProjectDefinedMappingLocation::Url { url, cache_ttl } => {
                                match (url.scheme(), cache_ttl) {
                                    ("file", _) => {
                                        let file_path = url.to_file_path().map_err(|_| {
                                            miette::miette!("{} is not a valid file url", url)
                                        })?;
                                        fetch_mapping_from_path(&file_path)?
                                    }
                                    (_, Some(ttl)) => {
                                        fetch_mapping_with_ttl(client, url, *ttl, cache_dir).await?
                                    }
                                    (_, None) => fetch_mapping_from_url(client, url).await?,
                                }
                            }
                            ProjectDefinedMappingLocation::Path(path) => {
                                fetch_mapping_from_path(path)?
                            }
                            ProjectDefinedMappingLocation::InMemory(mapping) => mapping.clone(),
                        };
                        merged.extend(mapping_by_name);
                    }

                    mapping_url_to_name.insert(
                        name.to_string(),
                        ResolvedChannelMapping {
                            mapping: merged,
                            mode: channel_mapping.mode,
                        },
                    );
                }

                Ok(mapping_url_to_name)
            })
            .await
            .cloned()
    }
}

/// Help text for failures to fetch a *user-configured* mapping location.
/// Unlike [`crate::MAPPING_OFFLINE_HELP`] this must not suggest "point at
/// your own mapping" — the user already did.
const LOCATION_FETCH_HELP: &str = "Check that the `location` URL in your `conda-pypi-map` entry is correct and reachable. \
     To tolerate temporary outages, add a `cache-ttl` so a previously fetched copy can be \
     reused, or use a local file instead.";

async fn fetch_mapping_from_url(
    client: &LazyClient,
    url: &Url,
) -> miette::Result<CompressedMapping> {
    let response = client
        .client()
        .get(url.clone())
        .send()
        .await
        .into_diagnostic()
        .wrap_err(miette::diagnostic!(
            help = LOCATION_FETCH_HELP,
            "failed to download conda-pypi mapping from {}",
            url.as_str()
        ))?;

    if !response.status().is_success() {
        return Err(miette::miette!(
            help = LOCATION_FETCH_HELP,
            "fetching the conda-pypi mapping from {} returned status {}",
            url.as_str(),
            response.status()
        ));
    }

    let mapping_by_name = response.json().await.into_diagnostic().context(format!(
        "failed to parse pypi name mapping located at {url}. Please make sure that it's a valid json"
    ))?;

    Ok(mapping_by_name)
}

/// Fetch a mapping from a url, caching it on disk for `ttl`.
///
/// A cached copy younger than `ttl` is used without touching the network.
/// When the refetch of an expired copy fails, the stale copy is used with a
/// warning so that solves keep working offline.
///
/// This is a small mtime-based file cache (the same pattern as the reverse
/// pypi-to-conda mapping cache in pixi-build-python) rather than the
/// `http-cache` middleware that already wraps the client: the middleware's
/// freshness is driven by server cache headers, its `max_ttl` is client-global
/// while `cache-ttl` is configured per mapping entry, and it has no
/// use-stale-on-error behavior.
async fn fetch_mapping_with_ttl(
    client: &LazyClient,
    url: &Url,
    ttl: Duration,
    cache_dir: &Path,
) -> miette::Result<CompressedMapping> {
    let cache_path = ttl_cache_path(cache_dir, url);

    if let Some((mapping, age)) = read_ttl_cache(&cache_path)
        && age < ttl
    {
        return Ok(mapping);
    }

    match fetch_mapping_from_url(client, url).await {
        Ok(mapping) => {
            write_ttl_cache(&cache_path, &mapping);
            Ok(mapping)
        }
        Err(err) => {
            // Fall back to a stale cached copy if we have one.
            if let Some((mapping, age)) = read_ttl_cache(&cache_path) {
                tracing::warn!(
                    "could not refresh conda-pypi mapping from {url}; using a cached copy that is {} old",
                    humantime::format_duration(Duration::from_secs(age.as_secs()))
                );
                Ok(mapping)
            } else {
                Err(err)
            }
        }
    }
}

/// The on-disk location of the TTL cache for a mapping url.
fn ttl_cache_path(cache_dir: &Path, url: &Url) -> PathBuf {
    let hash =
        rattler_digest::compute_bytes_digest::<rattler_digest::Sha256>(url.as_str().as_bytes());
    cache_dir
        .join(TTL_CACHE_SUBDIR)
        .join(format!("{hash:x}.json"))
}

/// Read a cached mapping and its age. Returns `None` if there is no cached
/// copy or it cannot be parsed.
fn read_ttl_cache(cache_path: &Path) -> Option<(CompressedMapping, Duration)> {
    let metadata = fs_err::metadata(cache_path).ok()?;
    // A modification time in the future (clock skew, NTP corrections) is
    // treated as age zero; returning `None` here would make a perfectly good
    // cached copy invisible to both the freshness check and the stale
    // fallback.
    let age = metadata.modified().ok().map(|modified| {
        SystemTime::now()
            .duration_since(modified)
            .unwrap_or(Duration::ZERO)
    })?;
    let content = fs_err::read_to_string(cache_path).ok()?;
    let mapping = serde_json::from_str(&content).ok()?;
    Some((mapping, age))
}

/// Write a mapping to the TTL cache. Failures are ignored; the cache is an
/// optimization.
fn write_ttl_cache(cache_path: &Path, mapping: &CompressedMapping) {
    let Some(parent) = cache_path.parent() else {
        return;
    };
    let _ = fs_err::create_dir_all(parent);
    let Ok(content) = serde_json::to_string(mapping) else {
        return;
    };
    // Write via a temporary file and rename so a concurrent reader never
    // observes a partially written cache file.
    let Ok(temp_file) = tempfile::NamedTempFile::new_in(parent) else {
        return;
    };
    if fs_err::write(temp_file.path(), content).is_ok() {
        let _ = temp_file.persist(cache_path);
    }
}

fn fetch_mapping_from_path(path: &Path) -> miette::Result<CompressedMapping> {
    let file = fs_err::File::open(path)
        .into_diagnostic()
        .context(format!("failed to open file {}", path.display()))?;
    let reader = std::io::BufReader::new(file);
    let mapping_by_name = serde_json::from_reader(reader)
        .into_diagnostic()
        .context(format!(
        "failed to parse pypi name mapping located at {}. Please make sure that it's a valid json",
        path.display()
    ))?;

    Ok(mapping_by_name)
}

/// This is a client that uses a project-defined in-memory mapping to derive purls.
#[derive(Default)]
pub(crate) struct ProjectDefinedResolver {
    mapping: MappingByChannel,
}

impl ProjectDefinedResolver {
    /// Returns the mapping associated with a channel.
    fn get_channel_mapping(&self, channel: &str) -> Option<&ResolvedChannelMapping> {
        self.mapping.get(normalize_channel(channel))
    }

    /// Returns the mapping mode that applies to the given record, or `None`
    /// if no project-defined mapping covers the record's channel.
    pub fn mode_for_record(&self, record: &RepoDataRecord) -> Option<MappingMode> {
        record
            .channel
            .as_ref()
            .and_then(|channel| self.get_channel_mapping(channel))
            .map(|mapping| mapping.mode)
    }
}

impl From<MappingByChannel> for ProjectDefinedResolver {
    fn from(value: MappingByChannel) -> Self {
        Self { mapping: value }
    }
}

impl ProjectDefinedResolver {
    pub(crate) async fn derive_project_defined_purls(
        &self,
        record: &RepoDataRecord,
        _cache_metrics: &CacheMetrics,
    ) -> Result<DerivationOutcome, MappingError> {
        let Some(channel) = record.channel.as_ref() else {
            return Ok(DerivationOutcome::NotApplicable);
        };

        // See if the mapping contains the channel
        let Some(project_defined_mapping) = self.get_channel_mapping(channel) else {
            return Ok(DerivationOutcome::NotApplicable);
        };

        // Find the mapping for this particular record
        match project_defined_mapping
            .mapping
            .get(record.package_record.name.as_normalized())
        {
            // The record is in the mapping with one or more pypi names
            Some(pypi_names) if !pypi_names.0.is_empty() => Ok(DerivationOutcome::Purls(
                pypi_names
                    .0
                    .iter()
                    .map(|name| {
                        pypi_purl(
                            name.clone(),
                            Some(PurlDerivationSource::ProjectDefinedMapping),
                        )
                    })
                    .collect(),
            )),
            // The record is in the mapping, but it has no pypi names
            Some(_) => Ok(DerivationOutcome::NoPurls),
            // The record is not in the mapping
            None => Ok(DerivationOutcome::NotApplicable),
        }
    }
}

#[cfg(test)]
mod test {
    use std::time::{Duration, SystemTime};

    use super::{read_ttl_cache, write_ttl_cache};
    use crate::PypiNames;

    fn write_cache_with_mtime(dir: &std::path::Path, age: i64) -> std::path::PathBuf {
        let path = dir.join("mapping.json");
        write_ttl_cache(
            &path,
            &[("foo".to_string(), PypiNames(vec!["bar".to_string()]))]
                .into_iter()
                .collect(),
        );
        let mtime = filetime::FileTime::from_system_time(if age >= 0 {
            SystemTime::now() - Duration::from_secs(age as u64)
        } else {
            SystemTime::now() + Duration::from_secs((-age) as u64)
        });
        filetime::set_file_mtime(&path, mtime).unwrap();
        path
    }

    #[test]
    fn test_read_ttl_cache_reports_age() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_cache_with_mtime(dir.path(), 7200);
        let (mapping, age) = read_ttl_cache(&path).expect("cache should be readable");
        assert_eq!(mapping["foo"], PypiNames(vec!["bar".to_string()]));
        // Allow some slack for slow filesystems.
        assert!(age >= Duration::from_secs(7100) && age < Duration::from_secs(7300));
    }

    #[test]
    fn test_read_ttl_cache_future_mtime_is_age_zero() {
        // A cache file with a modification time in the future (clock skew)
        // must still be readable, with age zero, so that both the freshness
        // check and the stale fallback can use it.
        let dir = tempfile::tempdir().unwrap();
        let path = write_cache_with_mtime(dir.path(), -3600);
        let (_, age) = read_ttl_cache(&path).expect("future-dated cache should be readable");
        assert_eq!(age, Duration::ZERO);
    }

    #[test]
    fn test_read_ttl_cache_missing_or_invalid() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_ttl_cache(&dir.path().join("missing.json")).is_none());

        let corrupt = dir.path().join("corrupt.json");
        fs_err::write(&corrupt, "not json").unwrap();
        assert!(read_ttl_cache(&corrupt).is_none());
    }
}
