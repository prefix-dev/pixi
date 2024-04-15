use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{PackageUrl, RepoDataRecord};
use reqwest_middleware::ClientWithMiddleware;
use serde::de::DeserializeOwned;
use std::{collections::HashMap, sync::Arc};
use url::Url;

use async_once_cell::OnceCell;

use crate::pypi_mapping::MappingLocation;

use super::{prefix_pypi_name_mapping, MappingMap, Reporter};

pub async fn fetch_mapping_from_url<T: DeserializeOwned>(
    client: &ClientWithMiddleware,
    url: &Url,
) -> miette::Result<T> {
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

    let mapping_by_name: T = response.json().await.into_diagnostic().context(format!(
        "failed to parse pypi name mapping located at {}. Please make sure that it's a valid json",
        url
    ))?;

    Ok(mapping_by_name)
}

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

                        let mapping_by_name = fetch_mapping_from_url(client, url).await?;

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
    conda_packages: &mut [RepoDataRecord],
    reporter: Option<Arc<dyn Reporter>>,
) -> miette::Result<()> {
    let packages_for_prefix_mapping: Vec<RepoDataRecord> = conda_packages
        .iter()
        .filter(|package| !mapping_url.contains_key(&package.channel))
        .cloned()
        .collect();

    let prefix_mapping = prefix_pypi_name_mapping::conda_pypi_name_mapping(
        client,
        &packages_for_prefix_mapping,
        reporter,
    )
    .await?;
    let compressed_mapping =
        prefix_pypi_name_mapping::conda_pypi_name_compressed_mapping(client).await?;

    let non_pypi_names_mapping = prefix_pypi_name_mapping::conda_non_pypi_names(client).await?;

    let custom_mapping = fetch_custom_mapping(client, mapping_url).await?;

    for record in conda_packages.iter_mut() {
        if !mapping_url.contains_key(&record.channel) {
            prefix_pypi_name_mapping::amend_pypi_purls_for_record(
                record,
                &prefix_mapping,
                &compressed_mapping,
                &non_pypi_names_mapping,
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
