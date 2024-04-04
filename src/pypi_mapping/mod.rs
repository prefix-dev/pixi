use std::{collections::HashMap, path::PathBuf, str::FromStr, sync::Arc};

use http_cache_reqwest::{CACacheManager, Cache, CacheMode, HttpCache, HttpCacheOptions};
use rattler_conda_types::{Channel, RepoDataRecord};
use reqwest_middleware::ClientBuilder;
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use url::Url;

use crate::config::get_cache_dir;

mod custom_pypi_mapping;
mod prefix_pypi_name_mapping;

pub trait Reporter: Send + Sync {
    fn download_started(&self, package: &RepoDataRecord, total: usize);
    fn download_finished(&self, package: &RepoDataRecord, total: usize);
    fn download_failed(&self, package: &RepoDataRecord, total: usize);
}

pub type ChannelName = String;

type MappingMap = HashMap<ChannelName, MappingLocation>;

#[derive(Debug)]
pub enum MappingLocation {
    Path(PathBuf),
    Url(Url),
}

pub enum MappingSource {
    Custom {
        mapping: MappingMap,
        default_conda_forge: Channel,
    },
    Prefix,
}

pub async fn amend_pypi_purls(
    client: reqwest::Client,
    mapping_source: MappingSource,
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
        MappingSource::Custom {
            mapping,
            default_conda_forge,
        } => {
            custom_pypi_mapping::amend_pypi_purls(
                &client,
                &mapping,
                default_conda_forge,
                conda_packages,
                reporter,
            )
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
