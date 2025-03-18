use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};

use dashmap::{DashMap, Entry};
use rattler_conda_types::{PackageUrl, RepoDataRecord};
use rattler_digest::Sha256Hash;
use reqwest::StatusCode;
use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{broadcast, Semaphore};

use crate::{CacheMetrics, DerivePurls, MappingError, PurlSource};

const STORAGE_URL: &str = "https://conda-mapping.prefix.dev";
const HASH_DIR: &str = "hash-v0";

/// Information about the pypi package a specific conda package is mapped to.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PackagePypiMapping {
    pub pypi_normalized_names: Option<Vec<String>>,
    pub versions: Option<HashMap<String, pep440_rs::Version>>,
    pub conda_name: String,
    pub package_name: String,
    pub direct_url: Option<Vec<String>>,
}

#[derive(Debug, Error)]
pub enum HashMappingClientError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Reqwest(#[from] reqwest_middleware::Error),
}

impl From<reqwest::Error> for HashMappingClientError {
    fn from(err: reqwest::Error) -> Self {
        HashMappingClientError::Reqwest(err.into())
    }
}

impl From<HashMappingClientError> for MappingError {
    fn from(value: HashMappingClientError) -> Self {
        match value {
            HashMappingClientError::Io(err) => MappingError::IoError(err),
            HashMappingClientError::Reqwest(err) => MappingError::Reqwest(err),
        }
    }
}

/// A client for fetching and caching the pypi name mapping from <https://conda-mapping.prefix.dev>.
///
/// This provides a hash based mapping to pypi packages which should yield
/// perfect results. The downside is that maybe not all packages are in the map.
/// Therefor, this client should always be combined with another fallback
/// client.
///
/// This client can be shared between multiple tasks. Individual requests are
/// coalesced. The client can cheaply be cloned.
#[derive(Clone)]
pub struct HashMappingClient {
    inner: Arc<HashMappingClientInner>,
}

struct HashMappingClientInner {
    client: ClientWithMiddleware,
    entries: DashMap<Sha256Hash, PendingOrFetched<Option<PackagePypiMapping>>>,
    limit: Option<Arc<Semaphore>>,
}

/// An entry that is either pending or has been fetched.
#[derive(Clone)]
enum PendingOrFetched<T> {
    Pending(Weak<broadcast::Sender<T>>),
    Fetched(T),
}

/// A builder for a `HashMappingClient`.
pub struct HashMappingClientBuilder {
    client: ClientWithMiddleware,
    limit: Option<Arc<Semaphore>>,
}

impl HashMappingClientBuilder {
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
    pub fn finish(self) -> HashMappingClient {
        HashMappingClient {
            inner: Arc::new(HashMappingClientInner {
                client: self.client,
                entries: DashMap::new(),
                limit: self.limit,
            }),
        }
    }
}

impl HashMappingClient {
    /// Constructs a new `HashMappingClient` with the provided
    /// `ClientWithMiddleware`.
    pub fn builder(client: ClientWithMiddleware) -> HashMappingClientBuilder {
        HashMappingClientBuilder {
            client,
            limit: None,
        }
    }

    /// Fetches the pypi name mapping and caches it to ensure that any
    /// subsequent request does not hit the network.
    pub async fn get_mapping(
        &self,
        sha256: Sha256Hash,
        cache_metrics: &CacheMetrics,
    ) -> Result<Option<PackagePypiMapping>, HashMappingClientError> {
        self.inner.get_mapping(sha256, cache_metrics).await
    }
}

impl HashMappingClientInner {
    /// Fetches the pypi name mapping and caches it to ensure that any
    /// subsequent request does not hit the network.
    pub async fn get_mapping(
        &self,
        sha256: Sha256Hash,
        cache_metrics: &CacheMetrics,
    ) -> Result<Option<PackagePypiMapping>, HashMappingClientError> {
        let sender = match self.entries.entry(sha256) {
            Entry::Vacant(entry) => {
                // Construct a sender so other tasks can subscribe
                let (sender, _) = broadcast::channel(1);
                let sender = Arc::new(sender);

                // Modify the current entry to the pending entry, this is an atomic operation
                // because who holds the entry holds mutable access.
                entry.insert(PendingOrFetched::Pending(Arc::downgrade(&sender)));

                sender
            }
            Entry::Occupied(mut entry) => {
                let subdir = entry.get();
                match subdir {
                    PendingOrFetched::Pending(sender) => {
                        let sender = sender.upgrade();

                        if let Some(sender) = sender {
                            // Create a receiver before we drop the entry. While we hold on to
                            // the entry we have exclusive access to it, this means the task
                            // currently fetching the mapping will not be able to store a value
                            // until we drop the entry.
                            // By creating the receiver here we ensure that we are subscribed
                            // before the other tasks sends a value over the channel.
                            let mut receiver = sender.subscribe();

                            // Explicitly drop the entry, so we don't block any other tasks.
                            drop(entry);
                            drop(sender);

                            // The sender is still active, so we can wait for the subdir to be
                            // created.
                            return match receiver.recv().await {
                                Ok(subdir) => Ok(subdir),
                                Err(_) => {
                                    // If this happens the sender was dropped.
                                    Err(std::io::Error::new(
                                        std::io::ErrorKind::Interrupted,
                                        "a coalesced request failed",
                                    )
                                    .into())
                                }
                            };
                        } else {
                            // Construct a sender so other tasks can subscribe
                            let (sender, _) = broadcast::channel(1);
                            let sender = Arc::new(sender);

                            // Modify the current entry to the pending entry, this is an atomic
                            // operation because who holds the entry holds mutable access.
                            entry.insert(PendingOrFetched::Pending(Arc::downgrade(&sender)));

                            sender
                        }
                    }
                    PendingOrFetched::Fetched(records) => return Ok(records.clone()),
                }
            }
        };

        // At this point we have exclusive write access to this specific entry. All
        // other tasks will find a pending entry and will wait for the records
        // to become available.
        //
        // Let's start by fetching the record. If an error occurs we immediately return
        // the error. This will drop the sender and all other waiting tasks will
        // receive an error.
        let mapping = {
            let _permit = match self.limit.as_ref() {
                Some(limit) => Some(
                    limit
                        .clone()
                        .acquire_owned()
                        .await
                        .expect("failed to acquire semaphore permit"),
                ),
                None => None,
            };
            try_fetch_mapping(&self.client, &sha256, cache_metrics).await?
        };

        // Store the fetched files in the entry.
        self.entries
            .insert(sha256, PendingOrFetched::Fetched(mapping.clone()));

        // Send the records to all waiting tasks. We don't care if there are no
        // receivers, so we drop the error.
        let _ = sender.send(mapping.clone());

        Ok(mapping)
    }
}

async fn try_fetch_mapping(
    client: &ClientWithMiddleware,
    sha256: &Sha256Hash,
    cache_metrics: &CacheMetrics,
) -> Result<Option<PackagePypiMapping>, HashMappingClientError> {
    let hash_str = format!("{:x}", sha256);
    let url = format!("{STORAGE_URL}/{HASH_DIR}/{}", hash_str);

    // Fetch the mapping from the server
    let response = client.get(&url).send().await?;

    cache_metrics.record_request_response(&response);

    // If no mapping was found for the hash, return None.
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }

    // Otherwise convert the response to a Package struct
    let package = response.json().await?;

    Ok(Some(package))
}

impl DerivePurls for HashMappingClient {
    async fn derive_purls(
        &self,
        record: &RepoDataRecord,
        cache_metrics: &CacheMetrics,
    ) -> Result<Option<Vec<PackageUrl>>, MappingError> {
        // Get the hash from the record, if there is no sha we cannot derive purls
        let Some(sha256) = record.package_record.sha256 else {
            return Ok(None);
        };

        // Fetch the mapping from the server, or return None if it doesn't exist
        let Some(mapped_package) = self.get_mapping(sha256, cache_metrics).await? else {
            return Ok(None);
        };

        // Get the pypi names from the mapping
        let Some(mapped_name) = mapped_package.pypi_normalized_names else {
            // If there are no pypi names, there are no purls
            return Ok(Some(vec![]));
        };

        Ok(Some(
            mapped_name
                .into_iter()
                .map(|pypi_name| {
                    let purl = PackageUrl::builder(String::from("pypi"), pypi_name)
                        .with_qualifier("source", PurlSource::HashMapping.as_str())
                        .expect("valid qualifier");
                    purl.build().expect("valid pypi package url")
                })
                .collect(),
        ))
    }
}
