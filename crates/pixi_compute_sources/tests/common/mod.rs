//! Helpers shared across the integration tests.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use fs_err as fs;
use pixi_compute_cache_dirs::{CacheDirs, CacheDirsKey};
use pixi_compute_engine::ComputeEngine;
use pixi_compute_env_vars::EnvVarsKey;
use pixi_compute_reporters::{OperationId, OperationIdSpawnHook};
use pixi_compute_sources::{
    GitCheckoutReporter, GitCheckoutSemaphore, UrlCheckoutReporter, UrlCheckoutSemaphore,
};
use pixi_path::{AbsPathBuf, AbsPresumedDirPathBuf};
use rattler_digest::{Sha256, Sha256Hash, digest::Digest};
use tempfile::TempDir;
use tokio::sync::Semaphore;
use url::Url;

/// Convert any path-like value into an absolute "presumed directory"
/// path. Tests use this for tempdirs and cache roots.
pub fn to_abs_dir(path: impl Into<PathBuf>) -> AbsPresumedDirPathBuf {
    AbsPathBuf::new(path)
        .expect("path is not absolute")
        .into_assume_dir()
}

/// Create a fresh tempdir under `CARGO_TARGET_TMPDIR`. Short prefix
/// keeps deeply nested paths under Windows' `MAX_PATH = 260`.
pub fn test_tempdir() -> TempDir {
    tempfile::Builder::new()
        .prefix("p-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("create test tempdir")
}

/// Stable sha256 used as the expected hash in cache-hit tests.
pub fn dummy_sha() -> Sha256Hash {
    Sha256::digest(b"pixi-url-cache-test")
}

/// Path to the small zip archive used as a stand-in for a remote URL
/// download. Lives at the repo root so the dispatcher's tests share it.
pub fn hello_world_archive() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/url/hello_world.zip")
}

/// Copy [`hello_world_archive`] into `tempdir` and return a `file://`
/// URL pointing at the copy. Used to fake a remote archive.
pub fn file_url_for_test(tempdir: &TempDir, name: &str) -> Url {
    let path = tempdir.path().join(name);
    fs::copy(hello_world_archive(), &path).unwrap();
    Url::from_file_path(&path).unwrap()
}

/// Pre-populate the URL checkout cache with the layout the URL Key
/// expects, so a `pin_and_checkout_url` call hits the cache instead of
/// trying to download.
pub fn prepare_cached_checkout(cache_root: &Path, sha: Sha256Hash) -> PathBuf {
    let checkout_dir = cache_root.join("checkouts").join(format!("{sha:x}"));
    fs::create_dir_all(&checkout_dir).unwrap();
    fs::write(checkout_dir.join("payload.txt"), "cached contents").unwrap();
    fs::write(checkout_dir.join(".pixi-url-ready"), "ready").unwrap();
    checkout_dir
}

/// Self-checking reporter that asserts `on_queued -> on_started ->
/// on_finished` fires exactly once in order. Implements both
/// [`GitCheckoutReporter`] and [`UrlCheckoutReporter`]; tests pass it
/// through whichever slot the Key reads.
pub struct LifecycleReporter {
    queued: AtomicBool,
    started: AtomicBool,
    finished: AtomicBool,
}

impl LifecycleReporter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            queued: AtomicBool::new(false),
            started: AtomicBool::new(false),
            finished: AtomicBool::new(false),
        })
    }

    /// Assert that all three callbacks fired. Call at the end of a
    /// test that ran exactly one checkout.
    pub fn assert_complete(&self) {
        assert!(self.queued.load(Ordering::SeqCst), "on_queued never fired");
        assert!(
            self.started.load(Ordering::SeqCst),
            "on_started never fired"
        );
        assert!(
            self.finished.load(Ordering::SeqCst),
            "on_finished never fired"
        );
    }

    fn record_queued(&self) -> OperationId {
        assert!(
            !self.queued.swap(true, Ordering::SeqCst),
            "on_queued fired more than once"
        );
        OperationId(0)
    }

    fn record_started(&self) {
        assert!(
            self.queued.load(Ordering::SeqCst),
            "on_started fired before on_queued"
        );
        assert!(
            !self.started.swap(true, Ordering::SeqCst),
            "on_started fired more than once"
        );
    }

    fn record_finished(&self) {
        assert!(
            self.started.load(Ordering::SeqCst),
            "on_finished fired before on_started"
        );
        assert!(
            !self.finished.swap(true, Ordering::SeqCst),
            "on_finished fired more than once"
        );
    }
}

impl GitCheckoutReporter for LifecycleReporter {
    fn on_queued(&self, _env: &pixi_git::resolver::RepositoryReference) -> OperationId {
        self.record_queued()
    }
    fn on_started(&self, _id: OperationId) {
        self.record_started();
    }
    fn on_finished(&self, _id: OperationId) {
        self.record_finished();
    }
}

impl UrlCheckoutReporter for LifecycleReporter {
    fn on_queued(&self, _env: &Url) -> OperationId {
        self.record_queued()
    }
    fn on_started(&self, _id: OperationId) {
        self.record_started();
    }
    fn on_finished(&self, _id: OperationId) {
        self.record_finished();
    }
}

/// Reporter that tracks the maximum number of URL checkouts in flight
/// at once. Used to verify the URL-checkout semaphore.
pub struct MaxInFlightReporter {
    next_id: AtomicU64,
    in_flight: AtomicUsize,
    max_seen: AtomicUsize,
}

impl MaxInFlightReporter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            next_id: AtomicU64::new(0),
            in_flight: AtomicUsize::new(0),
            max_seen: AtomicUsize::new(0),
        })
    }

    /// Highest concurrent in-flight count observed during the test.
    pub fn max_seen(&self) -> usize {
        self.max_seen.load(Ordering::SeqCst)
    }
}

impl UrlCheckoutReporter for MaxInFlightReporter {
    fn on_queued(&self, _env: &Url) -> OperationId {
        OperationId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }
    fn on_started(&self, _id: OperationId) {
        let cur = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_seen.fetch_max(cur, Ordering::SeqCst);
    }
    fn on_finished(&self, _id: OperationId) {
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Knobs accepted by [`build_test_engine`].
#[derive(Default)]
pub struct EngineConfig {
    pub cache_dirs: Option<CacheDirs>,
    pub git_reporter: Option<Arc<dyn GitCheckoutReporter>>,
    pub url_reporter: Option<Arc<dyn UrlCheckoutReporter>>,
    pub sequential: bool,
    pub max_concurrent_url: Option<usize>,
    pub max_concurrent_git: Option<usize>,
}

/// Build a [`ComputeEngine`] populated with the entries the
/// source-checkout Keys read: resolvers, download client, optional
/// reporter, optional semaphore, plus injected `CacheDirsKey` and
/// `EnvVarsKey`.
pub fn build_test_engine(config: EngineConfig) -> ComputeEngine {
    let cache_dirs = config
        .cache_dirs
        .unwrap_or_else(|| CacheDirs::new(to_abs_dir(test_tempdir().keep().join("pixi-cache"))));

    let mut builder = ComputeEngine::builder()
        .sequential_branches(config.sequential)
        .with_data(pixi_url::UrlResolver::default())
        .with_data(pixi_git::resolver::GitResolver::default())
        .with_data(rattler_networking::LazyClient::default())
        .with_spawn_hook(Arc::new(OperationIdSpawnHook));
    if let Some(reporter) = config.url_reporter {
        builder = builder.with_data(reporter);
    }
    if let Some(reporter) = config.git_reporter {
        builder = builder.with_data(reporter);
    }
    if let Some(n) = config.max_concurrent_url {
        builder = builder.with_data(UrlCheckoutSemaphore(Arc::new(Semaphore::new(n))));
    }
    if let Some(n) = config.max_concurrent_git {
        builder = builder.with_data(GitCheckoutSemaphore(Arc::new(Semaphore::new(n))));
    }
    let engine = builder.build();
    engine.inject(CacheDirsKey, Arc::new(cache_dirs));
    engine.inject(EnvVarsKey, Arc::new(std::collections::HashMap::new()));
    engine
}
