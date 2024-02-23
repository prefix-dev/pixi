use crate::config::get_cache_dir;
use async_once_cell::OnceCell;
use http_cache_reqwest::{CACacheManager, Cache, CacheMode, HttpCache, HttpCacheOptions};
use miette::{IntoDiagnostic, WrapErr};
use rattler_conda_types::{PackageUrl, RepoDataRecord};
use rattler_networking::retry_policies::ExponentialBackoff;
use reqwest::Client;
use reqwest_middleware::ClientBuilder;
use reqwest_retry::RetryTransientMiddleware;
use serde::Deserialize;
use std::{collections::HashMap, str::FromStr};
use url::Url;

#[derive(Deserialize)]
struct CondaPyPiNameMapping {
    conda_name: String,
    pypi_name: String,
}

/// Downloads and caches the conda-forge conda-to-pypi name mapping.
pub async fn conda_pypi_name_mapping() -> miette::Result<&'static HashMap<String, String>> {
    static MAPPING: OnceCell<HashMap<String, String>> = OnceCell::new();
    MAPPING.get_or_try_init(async {

        // Construct a client with a retry policy and local caching
        let retry_policy =
            ExponentialBackoff::builder().build_with_max_retries(3);
        let retry_strategy = RetryTransientMiddleware::new_with_policy(retry_policy);
        let cache_strategy = Cache(HttpCache {
            mode: CacheMode::Default,
            manager: CACacheManager { path: get_cache_dir().expect("missing cache directory").join("http-cache") },
            options: HttpCacheOptions::default(),
        });
        let client = ClientBuilder::new(Client::new())
            .with(cache_strategy)
            .with(retry_strategy)
            .build();

        let response = client.get("https://raw.githubusercontent.com/regro/cf-graph-countyfair/master/mappings/pypi/name_mapping.json").send().await
            .into_diagnostic()
            .context("failed to download pypi name mapping")?;
        let mapping: Vec<CondaPyPiNameMapping> = response
            .json()
            .await
            .into_diagnostic()
            .context("failed to parse pypi name mapping")?;
        let mapping_by_name: HashMap<_, _> = mapping
            .into_iter()
            .map(|m| (m.conda_name, m.pypi_name))
            .collect();
        Ok(mapping_by_name)
    }).await
}

/// Updates the specified repodata record to include an optional PyPI package name if it is missing.
///
/// This function guesses the PyPI package name from the conda package name if the record refers to
/// a conda-forge package.
pub fn amend_pypi_purls(
    record: &mut RepoDataRecord,
    conda_forge_mapping: &'static HashMap<String, String>,
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
        if let Some(mapped_name) =
            conda_forge_mapping.get(record.package_record.name.as_normalized())
        {
            record.package_record.purls.push(
                PackageUrl::new(String::from("pypi"), mapped_name).expect("valid pypi package url"),
            );
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
