use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::Arc,
};

use fs_err::{self, tokio as async_fs};
use futures::StreamExt;
use indicatif::ProgressBar;
use pixi_record::PinnedUrlSpec;
use pixi_spec::UrlSpec;
use pixi_utils::AsyncPrefixGuard;
use rattler_digest::{Md5, Md5Hash, Sha256, Sha256Hash, digest::Digest, parse_digest_from_hex};
use rattler_networking::LazyClient;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{instrument, warn};

use crate::{
    error::UrlError,
    extract,
    progress::{NoProgressHandler, ProgressHandler},
    util::{cache_digest, url_file_name},
};

const CHECKOUT_SENTINEL: &str = ".pixi-url-ready";
const CHECKOUT_MD5: &str = ".pixi-url-md5";

#[derive(Clone, Copy)]
struct CachedDigests {
    sha256: Sha256Hash,
    md5: Md5Hash,
}

/// A remote URL source that can be downloaded and extracted locally.
///
/// Layout inside the cache root for each URL digest:
/// - `archives/` stores raw downloaded archives as `{digest}-original-name`
/// - `checkouts/{sha256}` holds the extracted contents with `.pixi-url-ready` and
///   `.pixi-url-md5` markers
/// - `locks/{digest}` synchronizes downloads/extractions and contains `digests` metadata
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
            progress: Arc::new(NoProgressHandler),
        }
    }

    /// Set the [`ProgressHandler`] to use for downloads and extraction.
    #[must_use]
    pub fn with_progress_handler(mut self, handler: Arc<dyn ProgressHandler>) -> Self {
        self.progress = handler;
        self
    }

    /// Directory where raw downloaded archives are stored.
    fn archives_dir(&self) -> PathBuf {
        self.cache.join("archives")
    }

    /// Directory containing extracted archives keyed by sha256.
    fn checkouts_dir(&self) -> PathBuf {
        self.cache.join("checkouts")
    }

    /// Directory holding lock files and digest metadata per URL digest.
    fn locks_dir(&self) -> PathBuf {
        self.cache.join("locks")
    }

    /// Checkout directory for a specific sha256 hash.
    fn checkout_path(&self, sha: &Sha256Hash) -> PathBuf {
        self.checkouts_dir().join(format!("{sha:x}"))
    }

    /// Returns an existing checkout if it already satisfies the requested hashes.
    fn reuse_from_cache(
        &self,
        sha: &Sha256Hash,
        needs_md5: bool,
        cached_digests: Option<&CachedDigests>,
    ) -> Option<(PathBuf, Option<Md5Hash>)> {
        let path = self.checkout_path(sha);
        if !self.is_checkout_ready(&path) {
            return None;
        }

        let md5_from_cache = cached_digests
            .filter(|digests| &digests.sha256 == sha)
            .map(|digests| digests.md5);
        let md5 = md5_from_cache.or_else(|| self.checkout_md5(&path));

        if needs_md5 && md5.is_none() {
            return None;
        }

        Some((path, md5))
    }

    /// Reports whether a checkout directory completed extraction earlier.
    fn is_checkout_ready(&self, checkout: &Path) -> bool {
        checkout.exists() && checkout.join(CHECKOUT_SENTINEL).is_file()
    }

    /// Reads the stored md5 hash for a checkout directory.
    fn checkout_md5(&self, checkout: &Path) -> Option<Md5Hash> {
        let path = checkout.join(CHECKOUT_MD5);
        let contents = fs_err::read_to_string(path).ok()?;
        parse_digest_from_hex::<Md5>(contents.trim())
    }

    fn progress_bar(&self, prefix: &str, total: u64) -> ProgressBar {
        let bar = ProgressBar::new(total).with_style(self.progress.default_bytes_style());
        bar.set_prefix(prefix.to_string());
        self.progress.add_progress_bar(bar)
    }

    /// Fetch the URL, returning the extracted directory and pinned metadata.
    ///
    /// High-level overview of what we're doing here is:
    ///
    ///   - Ensure cache directories (archives/, checkouts/, locks/) exist for this URL digest.
    ///   - Look for a previously recorded sha256/md5 in locks/{digest}/digests; if present (or the caller supplied hashes), try to reuse an existing checkout (checkouts/{sha}/)
    ///     that has a .pixi-url-ready marker and matching hashes so we can return immediately.
    ///   - If reuse fails, acquire the per-URL lock (locks/{digest}) to prevent other processes from downloading/extracting the same archive concurrently. After locking, re-
    ///     check the reuse path in case another process finished first.
    ///   - Download the archive (or copy it for file:// URLs) into archives/{digest}-{filename}, streaming bytes through sha256/md5 hashers. Once finished, validate the computed
    ///     hashes against any caller-provided expectations.
    ///   - Extract the archive into checkouts/{sha}/, removing any stale directory first. After extraction, write .pixi-url-ready and .pixi-url-md5, and persist the sha256/md5
    ///     pair in locks/{digest}/digests for future reuse.
    ///   - Release the lock and return a Fetch struct pointing at the populated checkout plus a PinnedUrlSpec containing the precise hashes of the fetched archive.

    #[instrument(skip(self), fields(url = %self.spec.url))]
    pub async fn fetch(mut self) -> Result<Fetch, UrlError> {
        async_fs::create_dir_all(self.archives_dir()).await?;
        async_fs::create_dir_all(self.checkouts_dir()).await?;
        async_fs::create_dir_all(self.locks_dir()).await?;

        let url = self.spec.url.clone();
        let file_name = url_file_name(&url);
        if !extract::is_archive(&file_name) {
            return Err(UrlError::UnsupportedArchive(file_name));
        }

        let ident = cache_digest(&url);
        let mut cached_digests = self.read_cached_digests(&ident).await?;
        if self.spec.sha256.is_none() {
            if let Some(ref digests) = cached_digests {
                self.spec.sha256 = Some(digests.sha256);
            }
        }

        // Re-use existing checkouts if we already know the hash and it's available.
        if let Some(sha) = self.spec.sha256 {
            if let Some((path, md5)) =
                self.reuse_from_cache(&sha, self.spec.md5.is_some(), cached_digests.as_ref())
            {
                let pinned = PinnedUrlSpec {
                    url: url.clone(),
                    sha256: sha,
                    md5,
                };
                return Ok(Fetch { pinned, path });
            }
        }

        let lock_dir = self.locks_dir();
        let guard = AsyncPrefixGuard::new(&lock_dir.join(&ident)).await?;
        let mut write_guard = guard.write().await?;
        write_guard.begin().await?;

        cached_digests = self.read_cached_digests(&ident).await?;
        if self.spec.sha256.is_none() {
            if let Some(ref digests) = cached_digests {
                self.spec.sha256 = Some(digests.sha256);
            }
        }

        // Re-check after acquiring the lock to avoid duplicate work.
        if let Some(sha) = self.spec.sha256 {
            if let Some((path, md5)) =
                self.reuse_from_cache(&sha, self.spec.md5.is_some(), cached_digests.as_ref())
            {
                write_guard.finish().await?;
                let pinned = PinnedUrlSpec {
                    url,
                    sha256: sha,
                    md5,
                };
                return Ok(Fetch { pinned, path });
            }
        }

        let archive_name = format!("{ident}-{file_name}");
        let archive_path = self.archives_dir().join(archive_name);

        let (sha256, md5) = self.download_archive(&archive_path).await?;

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

        if let Some(expected) = self.spec.md5 {
            if md5 != expected {
                return Err(UrlError::Md5Mismatch {
                    url,
                    expected,
                    actual: md5,
                });
            }
        }

        let checkout_path = self.checkout_path(&sha256);
        if checkout_path.exists() {
            async_fs::remove_dir_all(&checkout_path).await?;
        }

        if let Err(err) = self.extract_archive(&archive_path, &checkout_path).await {
            let _ = async_fs::remove_dir_all(&checkout_path).await;
            return Err(err);
        }

        self.write_checkout_metadata(&checkout_path, &md5).await?;
        self.write_cached_digests(&ident, &sha256, &md5).await?;

        write_guard.finish().await?;

        let pinned = PinnedUrlSpec {
            url,
            sha256,
            md5: Some(md5),
        };

        Ok(Fetch {
            pinned,
            path: checkout_path,
        })
    }

    /// Streams the remote archive into the cache while hashing its content.
    async fn download_archive(
        &self,
        archive_path: &Path,
    ) -> Result<(Sha256Hash, Md5Hash), UrlError> {
        if let Some(parent) = archive_path.parent() {
            async_fs::create_dir_all(parent).await?;
        }

        if self.spec.url.scheme() == "file" {
            let source_path =
                self.spec.url.to_file_path().map_err(|_| {
                    std::io::Error::new(ErrorKind::InvalidInput, "invalid file url")
                })?;
            return self.copy_local_file(&source_path, archive_path).await;
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
        let mut md5 = Md5::default();

        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            sha.update(&chunk);
            md5.update(&chunk);
            progress_bar.inc(chunk.len() as u64);
        }
        file.flush().await?;
        progress_bar.finish_with_message("Downloaded");

        let sha256 = sha.finalize();
        let md5 = md5.finalize();

        Ok((sha256, md5))
    }

    /// Handles `file://` URLs by copying the archive locally and hashing it.
    async fn copy_local_file(
        &self,
        source: &Path,
        archive_path: &Path,
    ) -> Result<(Sha256Hash, Md5Hash), UrlError> {
        let mut reader = tokio::fs::File::open(source).await?;
        let mut writer = tokio::fs::File::create(archive_path).await?;
        let mut sha = Sha256::default();
        let mut md5 = Md5::default();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let read = reader.read(&mut buf).await?;
            if read == 0 {
                break;
            }
            writer.write_all(&buf[..read]).await?;
            sha.update(&buf[..read]);
            md5.update(&buf[..read]);
        }
        writer.flush().await?;
        Ok((sha.finalize(), md5.finalize()))
    }

    /// Extracts the downloaded archive into the checkout directory.
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

    /// Writes the readiness marker and md5 metadata next to the checkout.
    async fn write_checkout_metadata(
        &self,
        checkout: &Path,
        md5: &Md5Hash,
    ) -> Result<(), UrlError> {
        let marker = checkout.join(CHECKOUT_SENTINEL);
        if let Some(parent) = marker.parent() {
            async_fs::create_dir_all(parent).await?;
        }
        async_fs::write(marker, b"ready").await?;
        async_fs::write(checkout.join(CHECKOUT_MD5), format!("{md5:x}")).await?;
        Ok(())
    }

    /// Reads persisted sha256/md5 digests for a cached archive.
    async fn read_cached_digests(&self, ident: &str) -> Result<Option<CachedDigests>, UrlError> {
        let path = self.digests_path(ident);
        match async_fs::read_to_string(&path).await {
            Ok(contents) => {
                let mut lines = contents.lines();
                let Some(sha_line) = lines.next() else {
                    warn!("missing sha256 in cached digests {}", path.display());
                    return Ok(None);
                };
                let Some(md5_line) = lines.next() else {
                    warn!("missing md5 in cached digests {}", path.display());
                    return Ok(None);
                };

                let sha256 = match parse_digest_from_hex::<Sha256>(sha_line) {
                    Some(value) => value,
                    None => {
                        warn!("invalid sha256 digest for {}", path.display());
                        return Ok(None);
                    }
                };
                let md5 = match parse_digest_from_hex::<Md5>(md5_line) {
                    Some(value) => value,
                    None => {
                        warn!("invalid md5 digest for {}", path.display());
                        return Ok(None);
                    }
                };

                Ok(Some(CachedDigests { sha256, md5 }))
            }
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// Persists sha256/md5 digests for a freshly downloaded archive.
    async fn write_cached_digests(
        &self,
        ident: &str,
        sha256: &Sha256Hash,
        md5: &Md5Hash,
    ) -> Result<(), UrlError> {
        let path = self.digests_path(ident);
        if let Some(parent) = path.parent() {
            async_fs::create_dir_all(parent).await?;
        }
        let contents = format!("{sha256:x}\n{md5:x}");
        async_fs::write(path, contents).await?;
        Ok(())
    }

    /// Returns the filesystem path that stores digest metadata for an archive ident.
    fn digests_path(&self, ident: &str) -> PathBuf {
        self.locks_dir().join(ident).join("digests")
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
