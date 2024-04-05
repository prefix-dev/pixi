use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{Channel, PackageUrl, RepoDataRecord};
use reqwest_middleware::ClientWithMiddleware;
use std::{collections::HashMap, sync::Arc};

use async_once_cell::OnceCell;

use crate::pypi_mapping::MappingLocation;

use super::{prefix_pypi_name_mapping, MappingMap, Reporter};

pub async fn fetch_custom_mapping(
    client: &ClientWithMiddleware,
    mapping_url: &MappingMap,
) -> miette::Result<&'static HashMap<String, HashMap<String, String>>> {
    static MAPPING: OnceCell<HashMap<String, HashMap<String, String>>> = OnceCell::new();
    MAPPING
        .get_or_try_init(async {
            let mut mapping_url_to_name: HashMap<String, HashMap<String, String>> =
                Default::default();

            for (name, url) in mapping_url.iter() {
                // Fetch the mapping from the server or from the local

                match url {
                    MappingLocation::Url(url) => {
                        let response = client
                            .get(url.clone())
                            .send()
                            .await
                            .into_diagnostic()
                            .context("failed to download pypi mapping from custom location")?;

                        let mapping_by_name: HashMap<String, String> = response
                            .json()
                            .await
                            .into_diagnostic()
                            .context("failed to parse pypi name mapping")?;

                        mapping_url_to_name.insert(name.to_string(), mapping_by_name);
                    }
                    MappingLocation::Path(path) => {
                        let contents = std::fs::read_to_string(path)
                            .into_diagnostic()
                            .context(format!("mapping on {path:?} could not be loaded"))?;
                        let data: HashMap<String, String> = serde_json::from_str(&contents)
                            .unwrap_or_else(|_| {
                                panic!("Failed to parse JSON mapping located at {path:?}")
                            });

                        mapping_url_to_name.insert(name.to_string(), data);
                    }
                }
            }

            Ok(mapping_url_to_name)
        })
        .await
}

/// Amend the records with pypi purls if they are not present yet.
pub async fn amend_pypi_purls(
    client: &ClientWithMiddleware,
    mapping_url: &MappingMap,
    default_conda_forge_channel: Channel,
    conda_packages: &mut [RepoDataRecord],
    reporter: Option<Arc<dyn Reporter>>,
) -> miette::Result<()> {
    let conda_forge_name = default_conda_forge_channel.canonical_name();

    let conda_forge_packages: Vec<RepoDataRecord> = conda_packages
        .iter()
        .filter(|package| package.channel.contains(&conda_forge_name))
        .cloned()
        .collect();

    let prefix_mapping = if mapping_url.contains_key(&conda_forge_name) {
        None
    } else {
        Some(
            prefix_pypi_name_mapping::conda_pypi_name_mapping(
                client,
                &conda_forge_packages,
                reporter,
            )
            .await?,
        )
    };

    let custom_mapping = fetch_custom_mapping(client, mapping_url).await?;

    for record in conda_packages.iter_mut() {
        if record.channel.contains(&conda_forge_name)
            && !mapping_url.contains_key(&conda_forge_name)
        {
            // we need to use prefix conda-forge mapping for conda-forge
            // and for others channels to rely on the custom one
            prefix_pypi_name_mapping::amend_pypi_purls_for_record(
                record,
                prefix_mapping
                    .as_ref()
                    .expect("prefix-mapping already should be populated"),
            )?;
        } else {
            amend_pypi_purls_for_record(record, custom_mapping)?;
        }
    }

    Ok(())
}

/// Updates the specified repodata record to include an optional PyPI package name if it is missing.
///
/// This function guesses the PyPI package name from the conda package name if the record refers to
/// a conda-forge package.
fn amend_pypi_purls_for_record(
    record: &mut RepoDataRecord,
    custom_mapping: &'static HashMap<String, HashMap<String, String>>,
) -> miette::Result<()> {
    // If the package already has a pypi name we can stop here.
    if record
        .package_record
        .purls
        .iter()
        .any(|p| p.package_type() == "pypi")
    {
        return Ok(());
    }

    // If this package is a conda-forge package or user specified a custom channel mapping
    // we can try to guess the pypi name from the conda name
    if custom_mapping.contains_key(&record.channel) {
        tracing::warn!("record channel is {}", record.channel);
        if let Some(mapped_channel) = custom_mapping.get(&record.channel) {
            if let Some(mapped_name) =
                mapped_channel.get(record.package_record.name.as_normalized())
            {
                record.package_record.purls.push(
                    PackageUrl::new(String::from("pypi"), mapped_name)
                        .expect("valid pypi package url"),
                );
            }
        }
    }

    Ok(())
}
