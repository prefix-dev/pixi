use std::sync::Arc;

use async_once_cell::OnceCell;
use rattler_conda_types::{PackageUrl, RepoDataRecord};
use rattler_networking::LazyClient;
use tokio::sync::Semaphore;
use url::Url;

use crate::{
    CacheMetrics, CompressedMapping, DerivePurls, MappingError, PurlSource, is_conda_forge_record,
};

const COMPRESSED_MAPPING: &str =
    "https://raw.githubusercontent.com/prefix-dev/parselmouth/main/files/compressed_mapping.json";

/// A client for fetching and caching the compressed mapping from the
/// parselmouth github repository.
///
/// This mapping provides a mapping from the conda-forge package names to their
/// pypi counterparts, or `None` if the package is not a pypi package.
///
/// The downside of this client is that it only contains information for
/// conda-forge packages.
#[derive(Clone)]
pub struct CompressedMappingClient {
    inner: Arc<CompressedMappingClientInner>,
}

pub struct CompressedMappingClientBuilder {
    client: LazyClient,
    limit: Option<Arc<Semaphore>>,
}

struct CompressedMappingClientInner {
    client: LazyClient,
    mapping: OnceCell<CompressedMapping>,
    limit: Option<Arc<Semaphore>>,
}

impl CompressedMappingClientBuilder {
    /// Sets the concurrency limit for the client. This is useful to limit the
    /// maximum number of concurrent requests.
    pub fn with_concurrency_limit(self, limit: Arc<Semaphore>) -> Self {
        Self {
            limit: Some(limit),
            ..self
        }
    }

    /// Sets the concurrency limit for the client. This is useful to limit the
    /// maximum number of concurrent requests.
    pub fn set_concurrency_limit(&mut self, limit: Arc<Semaphore>) -> &mut Self {
        self.limit = Some(limit);
        self
    }

    /// Finish the construction of the client and return it.
    pub fn finish(self) -> CompressedMappingClient {
        CompressedMappingClient {
            inner: Arc::new(CompressedMappingClientInner {
                client: self.client,
                limit: self.limit,
                mapping: OnceCell::new(),
            }),
        }
    }
}

impl CompressedMappingClient {
    /// Constructs a new `HashMappingClient` with the provided
    /// `ClientWithMiddleware`.
    pub fn builder(client: LazyClient) -> CompressedMappingClientBuilder {
        CompressedMappingClientBuilder {
            client,
            limit: None,
        }
    }

    /// Fetches the compressed mapping and caches it to ensure that any
    /// subsequent request does not hit the network.
    pub async fn get_mapping(
        &self,
        cache_metrics: &CacheMetrics,
    ) -> Result<&CompressedMapping, reqwest_middleware::Error> {
        let inner = &self.inner;
        inner
            .mapping
            .get_or_try_init(async {
                let compressed_mapping_url = Url::parse(COMPRESSED_MAPPING)
                    .expect("COMPRESSED_MAPPING static variable should be valid");
                let _permit = match inner.limit.as_ref() {
                    Some(limit) => Some(
                        limit
                            .clone()
                            .acquire_owned()
                            .await
                            .expect("failed to acquire semaphore permit"),
                    ),
                    None => None,
                };
                let response = inner
                    .client
                    .client()
                    .get(compressed_mapping_url)
                    .send()
                    .await?
                    .error_for_status()
                    .map_err(reqwest_middleware::Error::from)?;

                cache_metrics.record_request_response(&response);

                response.json().await.map_err(Into::into)
            })
            .await
    }
}

impl DerivePurls for CompressedMappingClient {
    async fn derive_purls(
        &self,
        record: &RepoDataRecord,
        cache_metrics: &CacheMetrics,
    ) -> Result<Option<Vec<PackageUrl>>, MappingError> {
        // If the record does not refer to a conda-forge mapping we can skip it
        if !is_conda_forge_record(record) {
            return Ok(None);
        }

        // Get the mapping from the server
        let mapping = self.get_mapping(cache_metrics).await?;

        // Determine the mapping for the record
        let Some(potential_pypi_name) = mapping.get(record.package_record.name.as_normalized())
        else {
            return Ok(None);
        };

        // If the mapping is empty, there are no purls.
        let Some(pypi_name) = potential_pypi_name else {
            return Ok(Some(vec![]));
        };

        // Construct the purl
        let purl = PackageUrl::builder(String::from("pypi"), pypi_name)
            .with_qualifier("source", PurlSource::CompressedMapping.as_str())
            .expect("valid qualifier");
        let built_purl = purl.build().expect("valid pypi package url");

        Ok(Some(vec![built_purl]))
    }
}
