use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{PackageUrl, RepoDataRecord};
use reqwest_middleware::ClientWithMiddleware;
use std::collections::HashMap;

use async_once_cell::OnceCell;

use url::Url;

use crate::pypi_mapping::is_conda_forge_record;

pub async fn fetch_custom_mapping(
    client: &ClientWithMiddleware,
    mapping_url: Url,
) -> miette::Result<&'static HashMap<String, String>> {
    static MAPPING: OnceCell<HashMap<String, String>> = OnceCell::new();
    MAPPING
        .get_or_try_init(async {
            // Fetch the mapping from the server
            let response = client
                .get(mapping_url)
                .send()
                .await
                .into_diagnostic()
                .context("failed to download pypi mapping from custom location")?;

            let mapping_by_name: HashMap<String, String> = response
                .json()
                .await
                .into_diagnostic()
                .context("failed to parse pypi name mapping")?;

            Ok(mapping_by_name)
        })
        .await
}

/// Amend the records with pypi purls if they are not present yet.
pub async fn amend_pypi_purls(
    client: &ClientWithMiddleware,
    mapping_url: Url,
    conda_packages: &mut [RepoDataRecord],
) -> miette::Result<()> {
    let custom_mapping = fetch_custom_mapping(client, mapping_url).await?;
    for record in conda_packages.iter_mut() {
        amend_pypi_purls_for_record(record, custom_mapping)?;
    }
    Ok(())
}

/// Updates the specified repodata record to include an optional PyPI package name if it is missing.
///
/// This function guesses the PyPI package name from the conda package name if the record refers to
/// a conda-forge package.
fn amend_pypi_purls_for_record(
    record: &mut RepoDataRecord,
    custom_mapping: &'static HashMap<String, String>,
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

    // If this package is a conda-forge package we can try to guess the pypi name from the conda
    // name.
    if is_conda_forge_record(record) {
        if let Some(mapped_name) = custom_mapping.get(record.package_record.name.as_normalized()) {
            record.package_record.purls.push(
                PackageUrl::new(String::from("pypi"), mapped_name).expect("valid pypi package url"),
            );
        }
    }

    Ok(())
}
