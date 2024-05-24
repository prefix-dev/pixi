use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{PackageUrl, RepoDataRecord};
use reqwest_middleware::ClientWithMiddleware;
use url::Url;

use super::{
    build_pypi_purl_from_package_record, is_conda_forge_record, prefix_pypi_name_mapping,
    CustomMapping, Reporter,
};

pub async fn fetch_mapping_from_url<T>(
    client: &ClientWithMiddleware,
    url: &Url,
) -> miette::Result<T>
where
    T: serde::de::DeserializeOwned,
{
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

/// Amend the records with pypi purls if they are not present yet.
pub async fn amend_pypi_purls(
    client: &ClientWithMiddleware,
    mapping_url: &CustomMapping,
    conda_packages: &mut [RepoDataRecord],
    reporter: Option<Arc<dyn Reporter>>,
) -> miette::Result<()> {
    trim_conda_packages_channel_url_suffix(conda_packages);
    let packages_for_prefix_mapping: Vec<RepoDataRecord> = conda_packages
        .iter()
        .filter(|package| !mapping_url.mapping.contains_key(&package.channel))
        .cloned()
        .collect();

    let custom_mapping = mapping_url.fetch_custom_mapping(client).await?;

    // When all requested channels are present in the custom_mapping, we don't have
    // to request from the prefix_mapping. This will avoid fetching unwanted
    // URLs, e.g. behind corporate firewalls
    if packages_for_prefix_mapping.is_empty() {
        _amend_only_custom_pypi_purls(conda_packages, custom_mapping)?;
    } else {
        let prefix_mapping = prefix_pypi_name_mapping::conda_pypi_name_mapping(
            client,
            &packages_for_prefix_mapping,
            reporter,
        )
        .await?;
        let compressed_mapping =
            prefix_pypi_name_mapping::conda_pypi_name_compressed_mapping(client).await?;

        for record in conda_packages.iter_mut() {
            if !mapping_url.mapping.contains_key(&record.channel) {
                prefix_pypi_name_mapping::amend_pypi_purls_for_record(
                    record,
                    &prefix_mapping,
                    &compressed_mapping,
                )?;
            } else {
                amend_pypi_purls_for_record(record, custom_mapping)?;
            }
        }
    }

    Ok(())
}

/// Updates the specified repodata record to include an optional PyPI package
/// name if it is missing.
///
/// This function guesses the PyPI package name from the conda package name if
/// the record refers to a conda-forge package.
fn amend_pypi_purls_for_record(
    record: &mut RepoDataRecord,
    custom_mapping: &'static HashMap<String, HashMap<String, Option<String>>>,
) -> miette::Result<()> {
    // If the package already has a pypi name we can stop here.
    if record
        .package_record
        .purls
        .as_ref()
        .is_some_and(|vec| vec.iter().any(|p| p.package_type() == "pypi"))
    {
        return Ok(());
    }

    let mut not_a_pypi = false;
    let mut purls = Vec::new();

    // we verify if we have package channel and name in user provided mapping
    if let Some(mapped_channel) = custom_mapping.get(&record.channel) {
        if let Some(mapped_name) = mapped_channel.get(record.package_record.name.as_normalized()) {
            // we have a pypi name for it so we record a purl
            if let Some(name) = mapped_name {
                let purl = PackageUrl::builder(String::from("pypi"), name.to_string())
                    .with_qualifier("source", "project-defined-mapping")
                    .expect("valid qualifier");

                purls.push(purl.build().expect("valid pypi package url"));
            } else {
                not_a_pypi = true;
            }
        }
    }

    // if we don't have it and it's channel is conda-forge
    // we assume that it's the pypi package
    if !not_a_pypi && purls.is_empty() && is_conda_forge_record(record) {
        // Convert the conda package names to pypi package names. If the conversion
        // fails we just assume that its not a valid python package.
        if let Some(purl) = build_pypi_purl_from_package_record(&record.package_record) {
            purls.push(purl);
        }
    }

    let package_purls = record
        .package_record
        .purls
        .get_or_insert_with(BTreeSet::new);
    package_purls.extend(purls);

    Ok(())
}

pub fn _amend_only_custom_pypi_purls(
    conda_packages: &mut [RepoDataRecord],
    custom_mapping: &'static HashMap<String, HashMap<String, Option<String>>>,
) -> miette::Result<()> {
    for record in conda_packages.iter_mut() {
        amend_pypi_purls_for_record(record, custom_mapping)?;
    }
    Ok(())
}

fn trim_conda_packages_channel_url_suffix(conda_packages: &mut [RepoDataRecord]) {
    for package in conda_packages {
        package.channel = package.channel.trim_end_matches('/').to_string();
    }
}
