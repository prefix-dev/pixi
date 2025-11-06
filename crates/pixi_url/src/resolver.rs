use std::{path::PathBuf, sync::Arc};

use dashmap::{DashMap, mapref::one::Ref};
use pixi_spec::UrlSpec;
use rattler_digest::Sha256Hash;
use rattler_networking::LazyClient;
use url::Url;

use crate::{
    error::UrlError,
    progress::ProgressHandler,
    source::{Fetch, UrlSource},
};

/// Resolver that keeps track of precise hashes for URL sources.
#[derive(Default, Clone)]
pub struct UrlResolver(Arc<DashMap<Url, Sha256Hash>>);

impl UrlResolver {
    /// Inserts a known mapping between a URL and its sha256 hash.
    pub fn insert(&self, url: Url, sha: Sha256Hash) {
        self.0.insert(url, sha);
    }

    /// Returns the precise hash for the URL if it is known.
    fn get(&self, url: &Url) -> Option<Ref<Url, Sha256Hash>> {
        self.0.get(url)
    }

    /// Downloads and extracts the URL, caching the result on disk.
    pub async fn fetch(
        &self,
        mut spec: UrlSpec,
        client: LazyClient,
        cache: PathBuf,
        progress: Option<Arc<dyn ProgressHandler>>,
    ) -> Result<Fetch, UrlError> {
        if spec.sha256.is_none() {
            if let Some(precise) = self.get(&spec.url) {
                spec.sha256 = Some(*precise);
            }
        }

        let source = UrlSource::new(spec.clone(), client, cache);
        let source = if let Some(handler) = progress {
            source.with_progress_handler(handler)
        } else {
            source
        };

        let fetch = source.fetch().await?;
        self.insert(fetch.pinned().url.clone(), fetch.pinned().sha256);
        Ok(fetch)
    }
}
