//! Tracks what the prefix installer observed for each package download so a
//! fetch failure can be reported with request context.
//!
//! [`InstallerError::FailedToFetch`](rattler::install::InstallerError) carries
//! only the package identifier; the URL, the number of bytes that made it
//! across, and how long the attempt ran are all known to the install reporter
//! but never reach the error. [`FetchProgressReporter`] sits between rattler
//! and the (optional) real reporter, records that context per package, and
//! hands it back through [`FetchProgress`] once the install fails.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use rattler::install::{Reporter, Transaction};
use rattler_conda_types::{PrefixRecord, RepoDataRecord};
use rattler_redaction::Redact;
use url::Url;

/// What the installer observed while fetching one package.
#[derive(Debug, Clone)]
pub struct FetchAttempt {
    /// The URL the package was fetched from, with secrets redacted.
    pub url: Url,
    /// Size the repodata advertised, or the total the server reported. `None`
    /// when neither is known.
    pub expected: Option<u64>,
    /// Bytes received before the failure. Zero when the transfer never
    /// produced a progress report, e.g. because connecting failed.
    pub transferred: u64,
    /// How long the fetch ran before it was abandoned.
    pub elapsed: Duration,
}

/// Handle onto the state collected by a [`FetchProgressReporter`]. Cloneable
/// and safe to query after the reporter has been handed to the installer.
#[derive(Debug, Clone, Default)]
pub struct FetchProgress {
    state: Arc<Mutex<State>>,
}

impl FetchProgress {
    /// What was observed for `package`, identified the same way
    /// `InstallerError::FailedToFetch` identifies it. `None` when the package
    /// never reached the fetch stage.
    pub fn attempt(&self, package: &str) -> Option<FetchAttempt> {
        let state = lock(&self.state);
        let attempt = state.attempts.get(package)?;
        let started = attempt.started?;
        Some(FetchAttempt {
            url: redacted(&attempt.url),
            expected: attempt.expected,
            transferred: attempt.transferred,
            elapsed: started.elapsed(),
        })
    }
}

/// Removes credentials and signed parameters before displaying a URL.
fn redacted(url: &Url) -> Url {
    let mut url = url.clone().redact();
    let _ = url.set_password(None);
    if !url.username().is_empty() {
        let _ = url.set_username("");
    }
    url.set_query(None);
    url
}

#[derive(Debug, Default)]
struct State {
    /// Cache-entry index -> package identifier.
    entries: HashMap<usize, String>,
    /// Download index -> package identifier.
    downloads: HashMap<usize, String>,
    /// Package identifier -> what we have seen so far.
    attempts: HashMap<String, Attempt>,
}

#[derive(Debug)]
struct Attempt {
    url: Url,
    advertised_size: Option<u64>,
    expected: Option<u64>,
    transferred: u64,
    started: Option<Instant>,
}

/// A poisoned mutex only means some other thread panicked while holding it;
/// the collected progress is still readable and is only used for diagnostics.
fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Install reporter that records per-package fetch context and forwards every
/// call to the wrapped reporter, if there is one.
pub struct FetchProgressReporter {
    inner: Option<Box<dyn Reporter>>,
    state: Arc<Mutex<State>>,
}

impl FetchProgressReporter {
    /// Wraps `inner`, which may be absent when progress is not being
    /// displayed; the fetch context is collected either way.
    pub fn new(inner: Option<Box<dyn Reporter>>) -> (Self, FetchProgress) {
        let state = Arc::new(Mutex::new(State::default()));
        let progress = FetchProgress {
            state: state.clone(),
        };
        (Self { inner, state }, progress)
    }
}

impl Reporter for FetchProgressReporter {
    fn on_transaction_start(&self, transaction: &Transaction<PrefixRecord, RepoDataRecord>) {
        if let Some(inner) = &self.inner {
            inner.on_transaction_start(transaction);
        }
    }

    fn on_transaction_operation_start(&self, operation: usize) {
        if let Some(inner) = &self.inner {
            inner.on_transaction_operation_start(operation);
        }
    }

    fn on_populate_cache_start(&self, operation: usize, record: &RepoDataRecord) -> usize {
        // Fall back to the operation index when there is no inner reporter to
        // allocate one; rattler only feeds these values back to us.
        let cache_entry = self.inner.as_ref().map_or(operation, |inner| {
            inner.on_populate_cache_start(operation, record)
        });

        let package = record.identifier.to_string();
        let mut state = lock(&self.state);
        state.entries.insert(cache_entry, package.clone());
        state.attempts.insert(
            package,
            Attempt {
                url: record.url.clone(),
                advertised_size: record.package_record.size,
                expected: record.package_record.size,
                transferred: 0,
                started: None,
            },
        );
        cache_entry
    }

    fn on_validate_start(&self, cache_entry: usize) -> usize {
        self.inner
            .as_ref()
            .map_or(cache_entry, |inner| inner.on_validate_start(cache_entry))
    }

    fn on_validate_complete(&self, validate_idx: usize) {
        if let Some(inner) = &self.inner {
            inner.on_validate_complete(validate_idx);
        }
    }

    fn on_download_start(&self, cache_entry: usize) -> usize {
        let download_idx = self
            .inner
            .as_ref()
            .map_or(cache_entry, |inner| inner.on_download_start(cache_entry));

        let mut state = lock(&self.state);
        if let Some(package) = state.entries.get(&cache_entry).cloned() {
            // Time from here rather than from cache population so the elapsed
            // time reflects the transfer, not the cache validation before it.
            if let Some(attempt) = state.attempts.get_mut(&package) {
                attempt.started = Some(Instant::now());
                attempt.transferred = 0;
                attempt.expected = attempt.advertised_size;
            }
            state.downloads.insert(download_idx, package);
        }
        download_idx
    }

    fn on_download_progress(&self, download_idx: usize, progress: u64, total: Option<u64>) {
        let mut state = lock(&self.state);
        if let Some(package) = state.downloads.get(&download_idx).cloned()
            && let Some(attempt) = state.attempts.get_mut(&package)
        {
            attempt.transferred = progress;
            // The server's content length is more trustworthy than repodata.
            if total.is_some() {
                attempt.expected = total;
            }
        }
        drop(state);

        if let Some(inner) = &self.inner {
            inner.on_download_progress(download_idx, progress, total);
        }
    }

    fn on_download_completed(&self, download_idx: usize) {
        if let Some(inner) = &self.inner {
            inner.on_download_completed(download_idx);
        }
    }

    fn on_populate_cache_complete(&self, cache_entry: usize) {
        if let Some(inner) = &self.inner {
            inner.on_populate_cache_complete(cache_entry);
        }
    }

    fn on_unlink_start(&self, operation: usize, record: &PrefixRecord) -> usize {
        self.inner
            .as_ref()
            .map_or(operation, |inner| inner.on_unlink_start(operation, record))
    }

    fn on_unlink_complete(&self, index: usize) {
        if let Some(inner) = &self.inner {
            inner.on_unlink_complete(index);
        }
    }

    fn on_link_start(&self, operation: usize, record: &RepoDataRecord) -> usize {
        self.inner
            .as_ref()
            .map_or(operation, |inner| inner.on_link_start(operation, record))
    }

    fn on_link_complete(&self, index: usize) {
        if let Some(inner) = &self.inner {
            inner.on_link_complete(index);
        }
    }

    fn on_post_link_start(&self, package_name: &str, script_path: &str) -> usize {
        self.inner.as_ref().map_or(0, |inner| {
            inner.on_post_link_start(package_name, script_path)
        })
    }

    fn on_post_link_complete(&self, index: usize, success: bool) {
        if let Some(inner) = &self.inner {
            inner.on_post_link_complete(index, success);
        }
    }

    fn on_pre_unlink_start(&self, package_name: &str, script_path: &str) -> usize {
        self.inner.as_ref().map_or(0, |inner| {
            inner.on_pre_unlink_start(package_name, script_path)
        })
    }

    fn on_pre_unlink_complete(&self, index: usize, success: bool) {
        if let Some(inner) = &self.inner {
            inner.on_pre_unlink_complete(index, success);
        }
    }

    fn on_transaction_operation_complete(&self, operation: usize) {
        if let Some(inner) = &self.inner {
            inner.on_transaction_operation_complete(operation);
        }
    }

    fn on_transaction_complete(&self) {
        if let Some(inner) = &self.inner {
            inner.on_transaction_complete();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attempt(url: &str, transferred: u64, started: Option<Instant>) -> Attempt {
        Attempt {
            url: Url::parse(url).unwrap(),
            advertised_size: Some(100),
            expected: Some(100),
            transferred,
            started,
        }
    }

    #[test]
    fn redacts_private_channel_credentials() {
        let url = Url::parse("https://user:password@conda.anaconda.org/t/token/channel/pkg.conda")
            .unwrap();

        assert_eq!(
            redacted(&url).as_str(),
            "https://conda.anaconda.org/t/********/channel/pkg.conda"
        );
    }

    #[test]
    fn removes_signed_query_parameters() {
        let url = Url::parse(
            "https://dl.cloudsmith.io/signed/org/conda/package.conda?created=1&expires=2&signature=secret",
        )
        .unwrap();

        assert_eq!(
            redacted(&url).as_str(),
            "https://dl.cloudsmith.io/signed/org/conda/package.conda"
        );
    }

    #[test]
    fn ignores_failures_before_download_start() {
        let state = Arc::new(Mutex::new(State::default()));
        lock(&state).attempts.insert(
            "package".to_string(),
            attempt("https://example.com/package.conda", 0, None),
        );
        let progress = FetchProgress { state };

        assert!(progress.attempt("package").is_none());
    }

    #[test]
    fn download_retry_resets_transferred_bytes() {
        let state = Arc::new(Mutex::new(State::default()));
        {
            let mut state = lock(&state);
            state.entries.insert(7, "package".to_string());
            state.attempts.insert(
                "package".to_string(),
                attempt("https://example.com/package.conda", 0, None),
            );
        }
        let reporter = FetchProgressReporter {
            inner: None,
            state: Arc::clone(&state),
        };
        let progress = FetchProgress {
            state: Arc::clone(&state),
        };

        reporter.on_download_start(7);
        reporter.on_download_progress(7, 42, Some(120));
        reporter.on_download_start(7);

        let retry = progress.attempt("package").unwrap();
        assert_eq!(retry.transferred, 0);
        assert_eq!(retry.expected, Some(100));
    }
}
