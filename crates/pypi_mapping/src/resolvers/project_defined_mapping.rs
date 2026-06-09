use async_once_cell::OnceCell as AsyncCell;
use miette::{IntoDiagnostic, WrapErr};
use rattler_conda_types::RepoDataRecord;
use rattler_networking::LazyClient;
use std::path::Path;
use url::Url;

use crate::{
    CacheMetrics, CompressedMapping, MappingByChannel, MappingError, MappingMap,
    ProjectDefinedMappingLocation, PurlDerivationSource, channel::normalize_channel,
    derivation::DerivationOutcome, purl::pypi_purl,
};

/// Struct with a mapping of channel names to their respective mapping locations
/// location could be a remote url or local file.
///
/// This struct caches the mapping internally.
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

    /// Fetch the project-defined mapping from the server or load from the local
    pub async fn fetch_project_defined_mapping(
        &self,
        client: &LazyClient,
    ) -> miette::Result<MappingByChannel> {
        self.mapping_value
            .get_or_try_init(async {
                let mut mapping_url_to_name: MappingByChannel = Default::default();

                for (name, url) in self.mapping.iter() {
                    // Fetch the mapping from the server or from the local

                    match url {
                        ProjectDefinedMappingLocation::Url(url) => {
                            let mapping_by_name = match url.scheme() {
                                "file" => {
                                    let file_path = url.to_file_path().map_err(|_| {
                                        miette::miette!("{} is not a valid file url", url)
                                    })?;
                                    fetch_mapping_from_path(&file_path)?
                                }
                                _ => fetch_mapping_from_url(client, url).await?,
                            };

                            mapping_url_to_name.insert(name.to_string(), mapping_by_name);
                        }
                        ProjectDefinedMappingLocation::Path(path) => {
                            let mapping_by_name = fetch_mapping_from_path(path)?;

                            mapping_url_to_name.insert(name.to_string(), mapping_by_name);
                        }
                        ProjectDefinedMappingLocation::InMemory(mapping) => {
                            mapping_url_to_name.insert(name.to_string(), mapping.clone());
                        }
                    }
                }

                Ok(mapping_url_to_name)
            })
            .await
            .cloned()
    }
}

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
        .context(format!(
            "failed to download pypi mapping from {} location",
            url.as_str()
        ))?;

    if !response.status().is_success() {
        return Err(miette::miette!(
            "Could not request mapping located at {:?}",
            url.as_str()
        ));
    }

    let mapping_by_name = response.json().await.into_diagnostic().context(format!(
        "failed to parse pypi name mapping located at {url}. Please make sure that it's a valid json"
    ))?;

    Ok(mapping_by_name)
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

/// THis is a client that uses a project-defined in-memory mapping to derive purls.
#[derive(Default)]
pub(crate) struct ProjectDefinedResolver {
    mapping: MappingByChannel,
}

impl ProjectDefinedResolver {
    /// Returns the mapping associated with a channel.
    fn get_channel_mapping(&self, channel: &str) -> Option<&CompressedMapping> {
        self.mapping.get(normalize_channel(channel))
    }

    /// Returns true if this mapping applies to the given record.
    pub fn is_mapping_for_record(&self, record: &RepoDataRecord) -> bool {
        record
            .channel
            .as_ref()
            .is_some_and(|channel| self.get_channel_mapping(channel).is_some())
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
        match project_defined_mapping.get(record.package_record.name.as_normalized()) {
            // The record is in the mapping, and it has a pypi name
            Some(Some(mapped_name)) => Ok(DerivationOutcome::Purls(vec![pypi_purl(
                mapped_name.to_string(),
                Some(PurlDerivationSource::ProjectDefinedMapping),
            )])),
            Some(None) => {
                // The record is in the mapping, but it has no pypi name
                Ok(DerivationOutcome::NoPurls)
            }
            None => {
                // The record is not in the mapping
                Ok(DerivationOutcome::NotApplicable)
            }
        }
    }
}
