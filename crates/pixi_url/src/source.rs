use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use fs_err as fs;
use futures::StreamExt;
use indicatif::ProgressBar;
use pixi_record::PinnedUrlSpec;
use pixi_spec::UrlSpec;
use pixi_utils::AsyncPrefixGuard;
use rattler_digest::{Md5, Md5Hash, Sha256, Sha256Hash, digest::Digest};
use rattler_networking::LazyClient;
use tokio::io::AsyncWriteExt;
use tracing::instrument;

use crate::{
    error::UrlError,
    extract,
    progress::{NoProgressHandler, ProgressHandler},
    util::{cache_digest, url_file_name},
};

/// A remote URL source that can be downloaded and extracted locally.
pub struct UrlSource {
    spec: UrlSpec,
    client: LazyClient,
    cache: PathBuf,
    progress: Arc<dyn ProgressHandler>,
}

impl UrlSource {
    /// Initialize a new URL source.
    pub fn new(spec: UrlSpec, client: LazyClient, cache: impl Into<PathBuf>) -> Self {
        Self {
            spec,
            client,
            cache: cache.into(),
            progress: Arc::new(NoProgressHandler::default()),
        }
    }

    /// Set the [`ProgressHandler`] to use for downloads and extraction.
    #[must_use]
    pub fn with_progress_handler(mut self, handler: Arc<dyn ProgressHandler>) -> Self {
        self.progress = handler;
        self
    }

    fn archives_dir(&self) -> PathBuf {
        self.cache.join("archives")
    }

    fn checkouts_dir(&self) -> PathBuf {
        self.cache.join("checkouts")
    }

    fn locks_dir(&self) -> PathBuf {
        self.cache.join("locks")
    }

    fn checkout_path(&self, sha: &Sha256Hash) -> PathBuf {
        self.checkouts_dir().join(format!("{sha:x}"))
    }

    fn existing_checkout(&self, sha: &Sha256Hash) -> Option<PathBuf> {
        let path = self.checkout_path(sha);
        path.exists().then_some(path)
    }

    fn progress_bar(&self, prefix: &str, total: u64) -> ProgressBar {
        let bar = ProgressBar::new(total).with_style(self.progress.default_bytes_style());
        bar.set_prefix(prefix.to_string());
        self.progress.add_progress_bar(bar)
    }

    /// Fetch the URL, returning the extracted directory and pinned metadata.
    #[instrument(skip(self), fields(url = %self.spec.url))]
    pub async fn fetch(self) -> Result<Fetch, UrlError> {
        fs::create_dir_all(self.archives_dir())?;
        fs::create_dir_all(self.checkouts_dir())?;
        fs::create_dir_all(self.locks_dir())?;

        // Re-use existing checkouts if we already know the hash and it's available.
        if let Some(sha) = self.spec.sha256 {
            if let Some(path) = self.existing_checkout(&sha) {
                let pinned = PinnedUrlSpec {
                    url: self.spec.url.clone(),
                    sha256: sha,
                    md5: self.spec.md5,
                };
                return Ok(Fetch { pinned, path });
            }
        }

        let url = self.spec.url.clone();
        let ident = cache_digest(&url);
        let lock_dir = self.locks_dir();
        let guard = AsyncPrefixGuard::new(&lock_dir.join(&ident)).await?;
        let mut write_guard = guard.write().await?;
        write_guard.begin().await?;

        // Re-check after acquiring the lock to avoid duplicate work.
        if let Some(sha) = self.spec.sha256 {
            if let Some(path) = self.existing_checkout(&sha) {
                write_guard.finish().await?;
                let pinned = PinnedUrlSpec {
                    url,
                    sha256: sha,
                    md5: self.spec.md5,
                };
                return Ok(Fetch { pinned, path });
            }
        }

        let archive_name = format!("{}-{}", ident, url_file_name(&self.spec.url));
        let archive_path = self.archives_dir().join(archive_name);

        let (sha256, md5) = self
            .download_archive(&archive_path, self.spec.md5.is_some())
            .await?;

        let sha256 = match self.spec.sha256 {
            Some(expected) => {
                if sha256 != expected {
                    return Err(UrlError::Sha256Mismatch {
                        url,
                        expected,
                        actual: sha256,
                    });
                }
                expected
            }
            None => sha256,
        };

        let md5 = if let Some(expected) = self.spec.md5 {
            let actual = md5.expect("md5 hash computed when requested");
            if actual != expected {
                return Err(UrlError::Md5Mismatch {
                    url,
                    expected,
                    actual,
                });
            }
            Some(actual)
        } else {
            md5
        };

        let checkout_path = self.checkout_path(&sha256);
        if !checkout_path.exists() {
            self.extract_archive(&archive_path, &checkout_path).await?;
        }

        write_guard.finish().await?;

        let pinned = PinnedUrlSpec { url, sha256, md5 };

        Ok(Fetch {
            pinned,
            path: checkout_path,
        })
    }

    async fn download_archive(
        &self,
        archive_path: &Path,
        compute_md5: bool,
    ) -> Result<(Sha256Hash, Option<Md5Hash>), UrlError> {
        if let Some(parent) = archive_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let response = self
            .client
            .client()
            .get(self.spec.url.clone())
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(UrlError::HttpStatus {
                url: self.spec.url.clone(),
                status: response.status(),
            });
        }

        let total = response.content_length().unwrap_or(1);
        let progress_bar = self.progress_bar("Downloading", total);

        let mut file = tokio::fs::File::create(archive_path).await?;
        let mut sha = Sha256::default();
        let mut md5 = compute_md5.then(Md5::default);

        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            sha.update(&chunk);
            if let Some(ref mut md5_hasher) = md5 {
                md5_hasher.update(&chunk);
            }
            progress_bar.inc(chunk.len() as u64);
        }
        file.flush().await?;
        progress_bar.finish_with_message("Downloaded");

        let sha256 = sha.finalize();
        let md5 = md5.map(|hasher| hasher.finalize());

        Ok((sha256, md5))
    }

    async fn extract_archive(&self, archive_path: &Path, target: &Path) -> Result<(), UrlError> {
        let file_name = archive_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();

        let archive_path = archive_path.to_path_buf();
        let target = target.to_path_buf();
        let handler = self.progress.clone();

        tokio::task::spawn_blocking(move || -> Result<(), UrlError> {
            let handler_ref = handler.as_ref();
            if extract::is_tarball(&file_name) {
                extract::extract_tar(&archive_path, &target, handler_ref).map_err(UrlError::from)
            } else if file_name.ends_with(".zip") {
                extract::extract_zip(&archive_path, &target, handler_ref).map_err(UrlError::from)
            } else if file_name.ends_with(".7z") {
                extract::extract_7z(&archive_path, &target, handler_ref).map_err(UrlError::from)
            } else {
                Err(UrlError::UnsupportedArchive(file_name))
            }
        })
        .await??;

        Ok(())
    }
}

/// The result of downloading and extracting an URL.
#[derive(Debug, Clone)]
pub struct Fetch {
    pinned: PinnedUrlSpec,
    path: PathBuf,
}

impl Fetch {
    /// The pinned URL metadata.
    pub fn pinned(&self) -> &PinnedUrlSpec {
        &self.pinned
    }

    /// Directory containing the extracted sources.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Consume the fetch and return the extracted path.
    pub fn into_path(self) -> PathBuf {
        self.path
    }
}
