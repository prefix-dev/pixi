use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

use futures::{stream::FuturesUnordered, StreamExt};
use itertools::Itertools;
use miette::{IntoDiagnostic, WrapErr};
use rattler_conda_types::{PackageUrl, RepoDataRecord};
use rattler_digest::Sha256Hash;
use reqwest::StatusCode;
use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use url::Url;

use super::{
    build_pypi_purl_from_package_record, custom_pypi_mapping, is_conda_forge_record, PurlSource,
    Reporter,
};

const STORAGE_URL: &str = "https://conda-mapping.prefix.dev";
const HASH_DIR: &str = "hash-v0";
const COMPRESSED_MAPPING: &str =
    "https://raw.githubusercontent.com/prefix-dev/parselmouth/main/files/compressed_mapping.json";

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
        // because we later skip adding purls for packages
        // that have purls
        // here we only filter packages that don't them
        // to save some requests
        .filter(|package| package.package_record.purls.is_none())
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

        // Create a future that fetches the mapping for the record's hash concurrently
        // with the rest of the requests.
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

/// Downloads and caches prefix.dev conda-pypi mapping.
pub async fn conda_pypi_name_compressed_mapping(
    client: &ClientWithMiddleware,
) -> miette::Result<HashMap<String, Option<String>>> {
    let compressed_mapping_url =
        Url::parse(COMPRESSED_MAPPING).expect("COMPRESSED_MAPPING static variable should be valid");

    custom_pypi_mapping::fetch_mapping_from_url(client, &compressed_mapping_url).await
}

/// Amend the records with pypi purls if they are not present yet.
pub async fn amend_pypi_purls(
    client: &ClientWithMiddleware,
    conda_packages: &mut [RepoDataRecord],
    reporter: Option<Arc<dyn Reporter>>,
) -> miette::Result<()> {
    let conda_mapping = conda_pypi_name_mapping(client, conda_packages, reporter).await?;
    let compressed_mapping = conda_pypi_name_compressed_mapping(client).await?;

    for record in conda_packages.iter_mut() {
        amend_pypi_purls_for_record(record, &conda_mapping, &compressed_mapping)?;
    }

    Ok(())
}

/// Updates the specified repodata record to include an optional PyPI package
/// name if it is missing.
///
/// This function resolves package pypi purl using the following approach:
/// 1. Tries to find a mapping by package hash.
/// 2. If the mapping is missing, tries to find a .json mapping by name.
/// 3. If both mappings are missing and it's a conda-forge record, assumes it is
///    a PyPI package.
pub fn amend_pypi_purls_for_record(
    record: &mut RepoDataRecord,
    mapping_by_hash: &HashMap<Sha256Hash, Package>,
    compressed_mapping: &HashMap<String, Option<String>>,
) -> miette::Result<()> {
    // If we already figured out the pypi purls, we can skip this record.
    if record.package_record.purls.is_some() {
        return Ok(());
    }

    let mut purls = None;

    // if package have a hash
    if let Some(sha256) = record.package_record.sha256 {
        // we look into our mapping by it's hash
        if let Some(mapped_name) = mapping_by_hash.get(&sha256) {
            let purls = purls.get_or_insert_with(Vec::new);

            // if we have pypi names in mapping
            // we populate purls for it
            if let Some(pypi_names) = &mapped_name.pypi_normalized_names {
                for pypi_name in pypi_names {
                    let purl = PackageUrl::builder(String::from("pypi"), pypi_name)
                        .with_qualifier("source", PurlSource::HashMapping.as_str())
                        .expect("valid qualifier");
                    let built_purl = purl.build().expect("valid pypi package url");
                    // Push the value into the vector
                    purls.push(built_purl);
                }
            }
            // we don't have a mapping for it's hash yet
            // so we are looking into our .json map by name
        }
    }

    // if we don't have a mapping for it's hash yet
    // or this package is missing sha256
    // we are looking into our .json map by name
    if let Some(possible_mapped_name) =
        compressed_mapping.get(record.package_record.name.as_normalized())
    {
        if purls.is_none() && is_conda_forge_record(record) {
            let purls = purls.get_or_insert_with(Vec::new);

            // if we have a pypi name for it
            // we record the purl
            if let Some(mapped_name) = possible_mapped_name {
                let purl = PackageUrl::builder(String::from("pypi"), mapped_name)
                    .with_qualifier("source", PurlSource::CompressedMapping.as_str())
                    .expect("valid qualifier");
                let built_purl = purl.build().expect("valid pypi package url");
                purls.push(built_purl);
            }
        }
    }

    // package is not in our mapping yet
    // so we assume that it is the same as the one from conda-forge
    if let Some(purl) = assume_conda_is_pypi(purls.as_ref(), record) {
        purls.get_or_insert_with(Vec::new).push(purl);
    }

    // If we have found some purls we overwrite whatever was there before.
    if let Some(purls) = purls {
        record.package_record.purls = Some(BTreeSet::from_iter(purls));
    }

    Ok(())
}

/// Try to assume that the conda-forge package is a PyPI package and return a purl.
pub fn assume_conda_is_pypi(
    purls: Option<&Vec<PackageUrl>>,
    record: &RepoDataRecord,
) -> Option<PackageUrl> {
    if purls.is_none() && is_conda_forge_record(record) {
        // Convert the conda package names to pypi package names. If the conversion
        // fails we just assume that its not a valid python package.
        build_pypi_purl_from_package_record(&record.package_record)
    } else {
        None
    }
}
