use rattler_networking::LazyClient;
/// Derived from `uv-git` implementation
/// Source: https://github.com/astral-sh/uv/blob/4b8cc3e29e4c2a6417479135beaa9783b05195d3/crates/uv-git/src/source.rs
/// This module expose `GitSource` type that represents a remote Git source that
/// can be checked out locally.
use std::{
    borrow::Cow,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    sync::Arc,
};
use tracing::instrument;

use crate::{
    GitError, GitUrl, Reporter,
    credentials::GIT_STORE,
    git::GitRemote,
    resolver::RepositoryReference,
    sha::{GitOid, GitSha},
    url::RepositoryUrl,
};

/// Tri-state default for LFS fetching. Accepts `1`/`0`, `true`/`false`,
/// `yes`/`no`, `on`/`off` (case-insensitive). Unset/empty → `None`.
pub const PIXI_GIT_LFS_ENV: &str = "PIXI_GIT_LFS";

pub fn lfs_enabled_from_env() -> Option<bool> {
    let raw = std::env::var(PIXI_GIT_LFS_ENV).ok()?;
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }
    if value == "0"
        || value.eq_ignore_ascii_case("false")
        || value.eq_ignore_ascii_case("no")
        || value.eq_ignore_ascii_case("off")
    {
        return Some(false);
    }
    if value == "1"
        || value.eq_ignore_ascii_case("true")
        || value.eq_ignore_ascii_case("yes")
        || value.eq_ignore_ascii_case("on")
    {
        return Some(true);
    }
    tracing::warn!("unrecognised value for {PIXI_GIT_LFS_ENV}: {raw:?}; treating as enabled");
    Some(true)
}

/// A remote Git source that can be checked out locally.
pub struct GitSource {
    /// The Git reference from the manifest file.
    git: GitUrl,
    /// The HTTP client to use for fetching.
    client: LazyClient,
    /// The path to the Git source database.
    cache: PathBuf,
    /// The reporter to use for this source.
    reporter: Option<Arc<dyn Reporter>>,
    /// `Some(true)` = fetch LFS, `Some(false)` = skip + force-skip smudge,
    /// `None` = no opinion (don't touch `GIT_LFS_SKIP_SMUDGE`).
    lfs: Option<bool>,
}

impl GitSource {
    /// Initialize a new Git source.
    pub fn new(git: GitUrl, client: LazyClient, cache: impl Into<PathBuf>) -> Self {
        Self {
            git,
            client,
            cache: cache.into(),
            reporter: None,
            lfs: lfs_enabled_from_env(),
        }
    }

    /// Set the [`Reporter`] to use for the [`GitSource`].
    #[must_use]
    pub fn with_reporter(self, reporter: Arc<dyn Reporter>) -> Self {
        Self {
            reporter: Some(reporter),
            ..self
        }
    }

    /// Override the LFS preference. See [`PIXI_GIT_LFS_ENV`] for tri-state semantics.
    #[must_use]
    pub fn with_lfs(self, lfs: Option<bool>) -> Self {
        Self { lfs, ..self }
    }

    /// Fetch the underlying Git repository at the given revision.
    #[instrument(skip(self), fields(repository = %self.git.repository, rev = self.git.precise.map(tracing::field::display)))]
    pub fn fetch(self) -> Result<Fetch, GitError> {
        // Compute the canonical URL for the repository.
        let canonical = RepositoryUrl::new(&self.git.repository);

        // The path to the repo, within the Git database.
        let ident = cache_digest(&canonical);
        let db_path = self.cache.join("db").join(&ident);

        // Authenticate the URL, if necessary.
        let remote = if let Some(credentials) = GIT_STORE.get(&canonical) {
            Cow::Owned(credentials.apply(self.git.repository.clone()))
        } else {
            Cow::Borrowed(&self.git.repository)
        };

        let remote = GitRemote::new(&remote);
        let lfs_requested = self.lfs == Some(true);
        let (db, actual_rev, task) = match (self.git.precise, remote.db_at(&db_path).ok()) {
            // Cache hit: the DB has the commit and, if LFS was requested,
            // its LFS objects validate. Skip the regular fetch + checkout.
            (Some(rev), Some(db))
                if db.contains(rev.into())
                    && (!lfs_requested || db.contains_lfs_artifacts(rev.into())) =>
            {
                tracing::debug!(
                    "Using existing Git source `{}` pointed at `{}`",
                    self.git.repository,
                    rev
                );
                let db = db.with_lfs_ready(lfs_requested.then_some(true));
                (db, rev, None)
            }

            // ... otherwise we use this state to update the git database. Note
            // that we still check for being offline here, for example in the
            // situation that we have a locked revision but the database
            // doesn't have it.
            (locked_rev, db) => {
                tracing::debug!("Updating Git source `{}`", self.git.repository);

                // Report the checkout operation to the reporter.
                let task = self.reporter.as_ref().map(|reporter| {
                    reporter.on_checkout_start(remote.url(), self.git.reference.as_rev())
                });

                let (db, actual_rev) = remote.checkout(
                    &db_path,
                    db,
                    &self.git.reference,
                    locked_rev.map(GitOid::from),
                    &self.client,
                    self.lfs,
                )?;

                (db, GitSha::from(actual_rev), task)
            }
        };

        // Don’t use the full hash, in order to contribute less to reaching the
        // path length limit on Windows.
        let short_id = db.to_short_id(actual_rev.into())?;

        // Check out `actual_rev` from the database to a scoped location on the
        // filesystem. This will use hard links and such to ideally make the
        // checkout operation here pretty fast.
        let checkout_path = self
            .cache
            .join("checkouts")
            .join(&ident)
            .join(short_id.as_str());

        tracing::debug!(
            "Copying git revision `{}` to path `{}`",
            actual_rev,
            checkout_path.display()
        );
        db.copy_to(actual_rev.into(), &checkout_path, self.lfs)?;

        // Report the checkout operation to the reporter.
        if let Some(task) = task
            && let Some(reporter) = self.reporter.as_ref()
        {
            reporter.on_checkout_complete(remote.url(), short_id.as_str(), task);
        }

        tracing::trace!("Finished fetching Git source `{}`", self.git.repository);

        Ok(Fetch {
            repository: RepositoryReference {
                url: canonical,
                reference: self.git.reference.clone(),
            },
            commit: actual_rev,
            path: checkout_path,
            lfs_ready: db.lfs_ready() == Some(true),
        })
    }
}

#[derive(Debug, Clone)]
pub struct Fetch {
    /// The [`RepositoryReference`] reference that was fetched.
    repository: RepositoryReference,

    /// The precise git checkout
    commit: GitSha,

    /// The path to the checked-out repository.
    path: PathBuf,

    /// True iff LFS was requested for this fetch and `git lfs fsck` passed.
    /// `false` means either LFS wasn't requested or it wasn't ready.
    lfs_ready: bool,
}

impl Fetch {
    pub fn repository(&self) -> &RepositoryReference {
        &self.repository
    }

    pub fn commit(&self) -> GitSha {
        self.commit
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn into_path(self) -> PathBuf {
        self.path
    }

    pub fn lfs_ready(&self) -> &bool {
        &self.lfs_ready
    }
}

pub fn cache_digest(url: &RepositoryUrl) -> String {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    let hash = hasher.finish();
    format!("{hash:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialised env-var swap to keep parallel tests from racing.
    fn with_env<R>(value: Option<&str>, body: impl FnOnce() -> R) -> R {
        use std::sync::Mutex;
        static LOCK: Mutex<()> = Mutex::new(());
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::var(PIXI_GIT_LFS_ENV).ok();
        // SAFETY: tests are serialised by LOCK above.
        match value {
            Some(v) => unsafe { std::env::set_var(PIXI_GIT_LFS_ENV, v) },
            None => unsafe { std::env::remove_var(PIXI_GIT_LFS_ENV) },
        }
        let out = body();
        match previous {
            Some(v) => unsafe { std::env::set_var(PIXI_GIT_LFS_ENV, v) },
            None => unsafe { std::env::remove_var(PIXI_GIT_LFS_ENV) },
        }
        out
    }

    #[test]
    fn env_unset_is_none() {
        with_env(None, || assert_eq!(lfs_enabled_from_env(), None));
    }

    #[test]
    fn env_empty_is_none() {
        with_env(Some(""), || assert_eq!(lfs_enabled_from_env(), None));
        with_env(Some("   "), || assert_eq!(lfs_enabled_from_env(), None));
    }

    #[test]
    fn env_truthy_is_some_true() {
        for v in ["1", "true", "TRUE", "yes", "YES", "on", "ON"] {
            with_env(Some(v), || {
                assert_eq!(lfs_enabled_from_env(), Some(true), "value={v}")
            });
        }
    }

    #[test]
    fn env_falsy_is_some_false() {
        for v in ["0", "false", "FALSE", "no", "NO", "off", "OFF"] {
            with_env(Some(v), || {
                assert_eq!(lfs_enabled_from_env(), Some(false), "value={v}")
            });
        }
    }
}
