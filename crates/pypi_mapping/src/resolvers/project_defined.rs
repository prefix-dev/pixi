use async_once_cell::OnceCell as AsyncCell;
use miette::{IntoDiagnostic, WrapErr};
use rattler_conda_types::RepoDataRecord;
use rattler_networking::LazyClient;
use std::path::Path;
use url::Url;

use crate::{
    CacheMetrics, CompressedMapping, MappingByChannel, MappingError, MappingMap, MappingMode,
    ProjectDefinedMappingLocation, PurlDerivationSource, ResolvedChannelMapping,
    channel::normalize_channel, derivation::DerivationOutcome, purl::pypi_purl,
};

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
    ///
    /// Remote fetches go through the `http-cache` middleware that wraps
    /// `client` (`CacheMode::Default`): a fresh copy is served from disk, a
    /// stale one is revalidated, and — unless the response is `no-store` or
    /// `must-revalidate` — a stale copy is served when a refresh fails so
    /// solves keep working offline.
    pub async fn fetch_project_defined(
        &self,
        client: &LazyClient,
    ) -> miette::Result<MappingByChannel> {
        self.mapping_value
            .get_or_try_init(async {
                let mut mapping_url_to_name: MappingByChannel = Default::default();

                for (name, channel_mapping) in self.mapping.iter() {
                    let mut merged = CompressedMapping::default();
                    for source in &channel_mapping.sources {
                        let mapping_by_name = match source {
                            ProjectDefinedMappingLocation::Url { url } => {
                                if url.scheme() == "file" {
                                    let file_path = url.to_file_path().map_err(|_| {
                                        miette::miette!("{} is not a valid file url", url)
                                    })?;
                                    fetch_mapping_from_path(&file_path)?
                                } else {
                                    fetch_mapping_from_url(client, url).await?
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
                            same_name: channel_mapping.same_name,
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
     A previously fetched copy is reused automatically when a refresh fails, so this only \
     happens when there is no cached copy yet (or the server marked the response \
     `no-store`/`must-revalidate`); you can also use a local file instead.";

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

    let body = response
        .text()
        .await
        .into_diagnostic()
        .wrap_err(miette::diagnostic!(
            help = LOCATION_FETCH_HELP,
            "failed to download conda-pypi mapping from {}",
            url.as_str()
        ))?;

    parse_mapping_body(&body, url.as_str())
}

/// Parse a fetched mapping document. An HTML response (e.g. a GitHub `blob/`
/// page URL instead of the raw file) gets an explicit hint, because the bare
/// serde error ("expected value at line 1 column 1") does not tell the user
/// what went wrong.
fn parse_mapping_body(body: &str, source: &str) -> miette::Result<CompressedMapping> {
    serde_json::from_str(body).map_err(|err| {
        if body.trim_start().starts_with('<') {
            miette::miette!(
                help = "the response looks like an HTML page, not JSON. If this is a GitHub \
                        link, use the raw file URL (raw.githubusercontent.com) instead of the \
                        `blob/` page.",
                "failed to parse pypi name mapping located at {source}. Please make sure that \
                 it's a valid json: {err}"
            )
        } else {
            miette::miette!(
                "failed to parse pypi name mapping located at {source}. Please make sure that \
                 it's a valid json: {err}"
            )
        }
    })
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
pub(crate) struct ProjectDefined {
    mapping: MappingByChannel,
}

impl ProjectDefined {
    /// Returns the mapping associated with a channel.
    fn get_channel_mapping(&self, channel: &str) -> Option<&ResolvedChannelMapping> {
        self.mapping.get(normalize_channel(channel))
    }

    /// Returns the mapping behavior that applies to the given record, or
    /// `None` if no project-defined mapping covers the record's channel.
    pub fn behavior_for_record(&self, record: &RepoDataRecord) -> Option<(MappingMode, bool)> {
        record
            .channel
            .as_ref()
            .and_then(|channel| self.get_channel_mapping(channel))
            .map(|mapping| (mapping.mode, mapping.same_name))
    }
}

impl From<MappingByChannel> for ProjectDefined {
    fn from(value: MappingByChannel) -> Self {
        Self { mapping: value }
    }
}

impl ProjectDefined {
    pub(crate) async fn derive_project_defined_purls(
        &self,
        record: &RepoDataRecord,
        _cache_metrics: &CacheMetrics,
    ) -> Result<DerivationOutcome, MappingError> {
        let Some(channel) = record.channel.as_ref() else {
            return Ok(DerivationOutcome::NotApplicable);
        };

        // See if the mapping contains the channel
        let Some(project_defined) = self.get_channel_mapping(channel) else {
            return Ok(DerivationOutcome::NotApplicable);
        };

        // Find the mapping for this particular record
        match project_defined
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
    use super::parse_mapping_body;
    use crate::PypiNames;

    #[test]
    fn test_parse_mapping_body_html_gets_raw_url_hint() {
        let err = parse_mapping_body(
            "<!DOCTYPE html><html></html>",
            "https://github.com/org/repo/blob/main/mapping.json",
        )
        .unwrap_err();
        let help = err.help().expect("should carry a help text").to_string();
        assert!(help.contains("raw.githubusercontent.com"), "{help}");
    }

    #[test]
    fn test_parse_mapping_body_plain_json_error_has_no_html_hint() {
        let err = parse_mapping_body("not json", "https://example.com/m.json").unwrap_err();
        assert!(err.help().is_none());
        assert!(err.to_string().contains("https://example.com/m.json"));
    }

    #[test]
    fn test_parse_mapping_body_accepts_all_value_forms() {
        let mapping =
            parse_mapping_body(r#"{"a": "b", "c": ["d", "e"], "f": null}"#, "test").unwrap();
        assert_eq!(
            mapping["c"],
            PypiNames(vec!["d".to_string(), "e".to_string()])
        );
    }
}
