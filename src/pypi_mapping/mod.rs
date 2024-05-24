use std::{collections::HashMap, path::PathBuf, str::FromStr, sync::Arc};

use async_once_cell::OnceCell as AsyncCell;
use http_cache_reqwest::{CACacheManager, Cache, CacheMode, HttpCache, HttpCacheOptions};
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{PackageRecord, PackageUrl, RepoDataRecord};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use url::Url;

use crate::{config::get_cache_dir, pypi_mapping::custom_pypi_mapping::fetch_mapping_from_url};

pub mod custom_pypi_mapping;
pub mod prefix_pypi_name_mapping;

pub trait Reporter: Send + Sync {
    fn download_started(&self, package: &RepoDataRecord, total: usize);
    fn download_finished(&self, package: &RepoDataRecord, total: usize);
    fn download_failed(&self, package: &RepoDataRecord, total: usize);
}

pub type ChannelName = String;

pub type MappingMap = HashMap<ChannelName, MappingLocation>;
pub type MappingByChannel = HashMap<String, HashMap<String, Option<String>>>;

#[derive(Debug, Clone)]
pub enum MappingLocation {
    Path(PathBuf),
    Url(Url),
}

#[derive(Debug, Clone)]
/// Struct with a mapping of channel names to their respective mapping locations
/// location could be a remote url or local file
pub struct CustomMapping {
    pub mapping: MappingMap,
    mapping_value: Arc<AsyncCell<MappingByChannel>>,
}

impl CustomMapping {
    /// Create a new `CustomMapping` with the specified mapping.
    pub fn new(mapping: MappingMap) -> Self {
        Self {
            mapping,
            mapping_value: Default::default(),
        }
    }

    /// Fetch the custom mapping from the server or load from the local
    pub async fn fetch_custom_mapping(
        &self,
        client: &ClientWithMiddleware,
    ) -> miette::Result<MappingByChannel> {
        self.mapping_value
            .get_or_try_init(async {
                let mut mapping_url_to_name: MappingByChannel = Default::default();

                for (name, url) in self.mapping.iter() {
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
                            let data: HashMap<String, Option<String>> =
                                serde_json::from_str(&contents).into_diagnostic().context(
                                    format!(
                                        "Failed to parse JSON mapping located at {}",
                                        path.display()
                                    ),
                                )?;

                            mapping_url_to_name.insert(name.to_string(), data);
                        }
                    }
                }

                Ok(mapping_url_to_name)
            })
            .await
            .cloned()
    }
}

/// This enum represents the source of mapping
/// it can be user-defined ( custom )
/// or from prefix.dev ( prefix )
#[derive(Debug, Clone)]
pub enum MappingSource {
    Custom(CustomMapping),
    Prefix,
}

impl MappingSource {
    /// Return the custom `MappingMap`
    /// for `MappingSource::Custom`
    pub fn custom(&self) -> Option<CustomMapping> {
        match self {
            MappingSource::Custom(mapping) => Some(mapping.clone()),
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
        MappingSource::Custom(mapping) => {
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

/// Build a purl for a `PackageRecord`
/// it will return a purl in this format
/// `pkg:pypi/aiofiles`
pub fn build_pypi_purl_from_package_record(package_record: &PackageRecord) -> Option<PackageUrl> {
    let name = pep508_rs::PackageName::from_str(package_record.name.as_source()).ok();
    let version = pep440_rs::Version::from_str(&package_record.version.as_str()).ok();
    if let (Some(name), Some(_)) = (name, version) {
        let purl = PackageUrl::builder(String::from("pypi"), name.to_string());
        let built_purl = purl.build().expect("valid pypi package url");
        return Some(built_purl);
    }

    None
}
