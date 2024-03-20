use futures::{stream, StreamExt};
use http_cache_reqwest::{CACacheManager, Cache, CacheMode, HttpCache, HttpCacheOptions};
use miette::{IntoDiagnostic, WrapErr};
use rattler_conda_types::{PackageUrl, RepoDataRecord};
use reqwest_middleware::ClientBuilder;
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, str::FromStr};
use url::Url;

use crate::config::get_cache_dir;

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

/// Downloads and caches the conda-forge conda-to-pypi name mapping.
pub async fn conda_pypi_name_mapping(
    client: reqwest::Client,
    conda_packages: &[RepoDataRecord],
) -> miette::Result<HashMap<String, Package>> {
    // Construct a client with a retry policy and local caching
    let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);
    let retry_strategy = RetryTransientMiddleware::new_with_policy(retry_policy);
    let cache_strategy = Cache(HttpCache {
        mode: CacheMode::Default,
        manager: CACacheManager {
            path: get_cache_dir()
                .expect("missing cache directory")
                .join("http-cache"),
        },
        options: HttpCacheOptions::default(),
    });

    let client = ClientBuilder::new(client)
        .with(cache_strategy)
        .with(retry_strategy)
        .build();

    let filtered_packages: Vec<RepoDataRecord> = conda_packages
        .iter()
        .filter(|package| package.package_record.sha256.is_some())
        .cloned()
        .collect();

    let responses = stream::iter(filtered_packages)
        .map(|package| {
            let hash = package
                .package_record
                .sha256
                .expect("packages should be already filtered");
            let hash_str = format!("{:x}", hash);
            let client = &client;

            async move {
                let response = client
                    .get(format!("{STORAGE_URL}/{HASH_DIR}/{}", hash_str))
                    .send()
                    .await
                    .into_diagnostic()
                    .context("failed to download pypi name mapping")?;

                let package: Package = response
                    .json()
                    .await
                    .into_diagnostic()
                    .context("failed to parse pypi name mapping")?;

                Ok::<(String, Package), miette::ErrReport>((hash_str, package))
            }
        })
        .buffer_unordered(100);

    let mapping = responses
        .filter_map(|result| async move { result.ok() })
        .collect::<HashMap<_, _>>()
        .await;

    Ok(mapping)
}

/// Amend the records with pypi purls if they are not present yet.
pub async fn amend_pypi_purls(
    client: reqwest::Client,
    conda_packages: &mut [RepoDataRecord],
) -> miette::Result<()> {
    let conda_mapping = conda_pypi_name_mapping(client, conda_packages).await?;
    for record in conda_packages.iter_mut() {
        amend_pypi_purls_for_record(record, &conda_mapping)?;
    }
    Ok(())
}

/// Updates the specified repodata record to include an optional PyPI package name if it is missing.
///
/// This function guesses the PyPI package name from the conda package name if the record refers to
/// a conda-forge package.
fn amend_pypi_purls_for_record(
    record: &mut RepoDataRecord,
    conda_forge_mapping: &HashMap<String, Package>,
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
        let sha_str = format!("{:x}", sha256);
        if let Some(mapped_name) = conda_forge_mapping.get(&sha_str) {
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

/// Returns `true` if the specified record refers to a conda-forge package.
pub fn is_conda_forge_record(record: &RepoDataRecord) -> bool {
    Url::from_str(&record.channel).map_or(false, |u| is_conda_forge_url(&u))
}

/// Returns `true` if the specified url refers to a conda-forge channel.
pub fn is_conda_forge_url(url: &Url) -> bool {
    url.path().starts_with("/conda-forge")
}
