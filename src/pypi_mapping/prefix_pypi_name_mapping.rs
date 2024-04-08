use futures::stream::FuturesUnordered;
use futures::StreamExt;

use itertools::Itertools;
use miette::{IntoDiagnostic, WrapErr};
use rattler_conda_types::{PackageUrl, RepoDataRecord};
use rattler_digest::Sha256Hash;
use reqwest::StatusCode;
use reqwest_middleware::ClientWithMiddleware;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Semaphore;

use super::Reporter;

const STORAGE_URL: &str = "https://conda-mapping.prefix.dev";
const HASH_DIR: &str = "hash-v0";

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Package {
    pypi_normalized_names: Option<Vec<String>>,
    versions: Option<HashMap<String, pep440_rs::Version>>,
    conda_name: String,
    package_name: String,
    direct_url: Option<Vec<String>>,
}

async fn try_fetch_single_mapping(
    client: &ClientWithMiddleware,
    sha256: &Sha256Hash,
) -> miette::Result<Option<Package>> {
    let hash_str = format!("{:x}", sha256);

    // Fetch the mapping from the server
    let response = client
        .get(format!("{STORAGE_URL}/{HASH_DIR}/{}", hash_str))
        .send()
        .await
        .into_diagnostic()
        .context("failed to download pypi name mapping")?;

    // If no mapping was found for the hash, return None.
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }

    // Otherwise convert the response to a Package struct
    let package: Package = response
        .json()
        .await
        .into_diagnostic()
        .context("failed to parse pypi name mapping")?;

    Ok(Some(package))
}

/// Downloads and caches the conda-forge conda-to-pypi name mapping.
pub async fn conda_pypi_name_mapping(
    client: &ClientWithMiddleware,
    conda_packages: &[RepoDataRecord],
    reporter: Option<Arc<dyn Reporter>>,
) -> miette::Result<HashMap<Sha256Hash, Package>> {
    let filtered_packages = conda_packages
        .iter()
        .filter_map(|package| {
            package
                .package_record
                .sha256
                .as_ref()
                .map(|hash| (package, *hash))
        })
        .collect_vec();

    let total_records = filtered_packages.len();
    let mut pending_futures = FuturesUnordered::new();
    let concurrency_limit = Arc::new(Semaphore::new(100));
    for (record, hash) in filtered_packages {
        if let Some(reporter) = &reporter {
            reporter.download_started(record, total_records);
        }

        let client = client.clone();
        let reporter = reporter.clone();
        let concurrency_limit = concurrency_limit.clone();

        // Create a future that fetches the mapping for the record's hash concurrently with the rest of the requests.
        pending_futures.push(async move {
            // Acquire a permit to limit the number of concurrent requests
            let _permit = concurrency_limit
                .acquire_owned()
                .await
                .expect("semaphore error");

            // Fetch the mapping by the hash of the record.
            let result = try_fetch_single_mapping(&client, &hash).await;

            // Report the result to the reporter
            if let Some(reporter) = reporter {
                match &result {
                    Ok(_) => reporter.download_finished(record, total_records),
                    Err(_) => reporter.download_failed(record, total_records),
                }
            }

            match result {
                Ok(Some(package)) => Ok(Some((hash, package))),
                Ok(None) => Ok(None),
                Err(e) => Err(e),
            }
        });
    }

    let mut result_map = HashMap::with_capacity(total_records);
    while let Some(result) = pending_futures.next().await {
        match result {
            Ok(Some((hash, package))) => {
                // Add the mapping to the result hashmap
                result_map.insert(hash, package);
            }
            Ok(None) => {
                // If no mapping was found, do nothing.
            }
            Err(e) => {
                // If an error occurred, bail out,.
                return Err(e);
            }
        }
    }

    Ok(result_map)
}

/// Amend the records with pypi purls if they are not present yet.
pub async fn amend_pypi_purls(
    client: &ClientWithMiddleware,
    conda_packages: &mut [RepoDataRecord],
    reporter: Option<Arc<dyn Reporter>>,
) -> miette::Result<()> {
    let conda_mapping = conda_pypi_name_mapping(client, conda_packages, reporter).await?;
    for record in conda_packages.iter_mut() {
        amend_pypi_purls_for_record(record, &conda_mapping)?;
    }

    Ok(())
}

/// Updates the specified repodata record to include an optional PyPI package name if it is missing.
///
/// This function guesses the PyPI package name from the conda package name if the record refers to
/// a conda-forge package.
pub fn amend_pypi_purls_for_record(
    record: &mut RepoDataRecord,
    conda_forge_mapping: &HashMap<Sha256Hash, Package>,
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

    if let Some(sha256) = record.package_record.sha256 {
        if let Some(mapped_name) = conda_forge_mapping.get(&sha256) {
            if let Some(pypi_names_with_versions) = &mapped_name.versions {
                for (pypi_name, pypi_version) in pypi_names_with_versions {
                    let mut purl = PackageUrl::builder(String::from("pypi"), pypi_name);
                    // sometimes packages are mapped to 0.0.0
                    // we don't want this version because we can't prove if it's actual or not
                    let pypi_version_str = pypi_version.to_string();
                    if pypi_version_str != "0.0.0" {
                        purl = purl.with_version(pypi_version_str);
                    };

                    let built_purl = purl.build().expect("valid pypi package url and version");
                    record.package_record.purls.push(built_purl);
                }
            }
        }
    }

    Ok(())
}
