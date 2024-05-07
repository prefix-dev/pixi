use std::{collections::HashMap, path::PathBuf, str::FromStr, sync::Arc};

use http_cache_reqwest::{CACacheManager, Cache, CacheMode, HttpCache, HttpCacheOptions};
use rattler_conda_types::{PackageRecord, PackageUrl, RepoDataRecord};
use reqwest_middleware::ClientBuilder;
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use url::Url;

use crate::config::get_cache_dir;

pub mod custom_pypi_mapping;
pub mod prefix_pypi_name_mapping;

pub trait Reporter: Send + Sync {
    fn download_started(&self, package: &RepoDataRecord, total: usize);
    fn download_finished(&self, package: &RepoDataRecord, total: usize);
    fn download_failed(&self, package: &RepoDataRecord, total: usize);
}

pub type ChannelName = String;

pub type MappingMap = HashMap<ChannelName, MappingLocation>;

#[derive(Debug, Clone)]
pub enum MappingLocation {
    Path(PathBuf),
    Url(Url),
}

pub enum MappingSource {
    Custom { mapping: MappingMap },
    Prefix,
}

impl MappingSource {
    pub fn custom(&self) -> Option<MappingMap> {
        match self {
            MappingSource::Custom { mapping } => Some(mapping.clone()),
            _ => None,
        }
    }
}

pub async fn amend_pypi_purls(
    client: reqwest::Client,
    mapping_source: &MappingSource,
    conda_packages: &mut [RepoDataRecord],
    reporter: Option<Arc<dyn Reporter>>,
) -> miette::Result<()> {
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

    match mapping_source {
        MappingSource::Custom { mapping } => {
            custom_pypi_mapping::amend_pypi_purls(&client, mapping, conda_packages, reporter)
                .await?;
        }
        MappingSource::Prefix => {
            prefix_pypi_name_mapping::amend_pypi_purls(&client, conda_packages, reporter).await?;
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

pub fn build_pypi_purl_from_package_record(package_record: &PackageRecord) -> Option<PackageUrl> {
    let name = pep508_rs::PackageName::from_str(package_record.name.as_source()).ok();
    let version = pep440_rs::Version::from_str(&package_record.version.as_str()).ok();
    if let (Some(name), Some(_)) = (name, version) {
        let mut purl = PackageUrl::builder(String::from("pypi"), name.to_string());
        purl = purl
            .with_qualifier("source", "conda-forge-mapping")
            .expect("valid qualifier");
        let built_purl = purl.build().expect("valid pypi package url");
        return Some(built_purl);
    }

    None
}
