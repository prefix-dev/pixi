use std::path::Path;

use async_once_cell::OnceCell as AsyncCell;
use miette::{IntoDiagnostic, WrapErr};
use reqwest_middleware::ClientWithMiddleware;
use url::Url;

use crate::{CompressedMapping, MappingByChannel, MappingLocation, MappingMap};

/// Struct with a mapping of channel names to their respective mapping locations
/// location could be a remote url or local file.
///
/// This struct caches the mapping internally.
#[derive(Debug)]
pub struct CustomMapping {
    pub mapping: MappingMap,
    mapping_value: AsyncCell<MappingByChannel>,
}

impl CustomMapping {
    /// Create a new `CustomMapping` with the specified mapping.
    pub fn new(mapping: MappingMap) -> Self {
        Self {
            mapping,
            mapping_value: Default::default(),
        }
    }

    /// Fetch the custom mapping from the server or load from the local
    pub async fn fetch_custom_mapping(
        &self,
        client: &ClientWithMiddleware,
    ) -> miette::Result<MappingByChannel> {
        self.mapping_value
            .get_or_try_init(async {
                let mut mapping_url_to_name: MappingByChannel = Default::default();

                for (name, url) in self.mapping.iter() {
                    // Fetch the mapping from the server or from the local

                    match url {
                        MappingLocation::Url(url) => {
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
                        MappingLocation::Path(path) => {
                            let mapping_by_name = fetch_mapping_from_path(path)?;

                            mapping_url_to_name.insert(name.to_string(), mapping_by_name);
                        }
                        MappingLocation::Memory(mapping) => {
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
    client: &ClientWithMiddleware,
    url: &Url,
) -> miette::Result<CompressedMapping> {
    let response = client
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
        "failed to parse pypi name mapping located at {}. Please make sure that it's a valid json",
        url
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
