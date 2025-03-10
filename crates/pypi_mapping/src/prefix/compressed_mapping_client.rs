use std::sync::Arc;

use async_once_cell::OnceCell;
use reqwest_middleware::ClientWithMiddleware;
use tokio::sync::Semaphore;
use url::Url;

use crate::CompressedMapping;

const COMPRESSED_MAPPING: &str =
    "https://raw.githubusercontent.com/prefix-dev/parselmouth/main/files/compressed_mapping.json";

/// A client for fetching and caching the compressed mapping from the
/// parselmouth repository.
#[derive(Clone)]
pub struct CompressedMappingClient {
    inner: Arc<CompressedMappingClientInner>,
}

pub struct CompressedMappingClientBuilder {
    client: ClientWithMiddleware,
    limit: Option<Arc<Semaphore>>,
}

struct CompressedMappingClientInner {
    client: ClientWithMiddleware,
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
    pub fn builder(client: ClientWithMiddleware) -> CompressedMappingClientBuilder {
        CompressedMappingClientBuilder {
            client,
            limit: None,
        }
    }

    /// Fetches the compressed mapping and caches it to ensure that any
    /// subsequent request does not hit the network.
    pub async fn get_mapping(&self) -> Result<&CompressedMapping, reqwest_middleware::Error> {
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
                inner
                    .client
                    .get(compressed_mapping_url)
                    .send()
                    .await?
                    .error_for_status()
                    .map_err(reqwest_middleware::Error::from)?
                    .json()
                    .await
                    .map_err(Into::into)
            })
            .await
    }
}
