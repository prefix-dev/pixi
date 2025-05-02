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

use reqwest_middleware::ClientWithMiddleware;
use tracing::instrument;

use crate::{
    GitError, GitUrl, Reporter,
    credentials::GIT_STORE,
    git::GitRemote,
    resolver::RepositoryReference,
    sha::{GitOid, GitSha},
    url::RepositoryUrl,
};

/// A remote Git source that can be checked out locally.
pub struct GitSource {
    /// The Git reference from the manifest file.
    git: GitUrl,
    /// The HTTP client to use for fetching.
    client: ClientWithMiddleware,
    /// The path to the Git source database.
    cache: PathBuf,
    /// The reporter to use for this source.
    reporter: Option<Arc<dyn Reporter>>,
}

impl GitSource {
    /// Initialize a new Git source.
    pub fn new(
        git: GitUrl,
        client: impl Into<ClientWithMiddleware>,
        cache: impl Into<PathBuf>,
    ) -> Self {
        Self {
            git,
            client: client.into(),
            cache: cache.into(),
            reporter: None,
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

    /// Fetch the underlying Git repository at the given revision.
    #[instrument(skip(self), fields(repository = %self.git.repository, rev = ?self.git.precise))]
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
        let (db, actual_rev, task) = match (self.git.precise, remote.db_at(&db_path).ok()) {
            // If we have a locked revision, and we have a preexisting database
            // which has that revision, then no update needs to happen.
            (Some(rev), Some(db)) if db.contains(rev.into()) => {
                tracing::debug!(
                    "Using existing Git source `{}` pointed at `{}`",
                    self.git.repository,
                    rev
                );
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
            "Copying git revision {:?} to path {:?}",
            actual_rev,
            checkout_path
        );
        db.copy_to(actual_rev.into(), &checkout_path)?;

        // Report the checkout operation to the reporter.
        if let Some(task) = task {
            if let Some(reporter) = self.reporter.as_ref() {
                reporter.on_checkout_complete(remote.url(), short_id.as_str(), task);
            }
        }

        tracing::debug!("Finished fetching Git source `{}`", self.git.repository);

        Ok(Fetch {
            repository: RepositoryReference {
                url: canonical,
                reference: self.git.reference.clone(),
            },
            url: self.git.clone(),
            commit: actual_rev,
            path: checkout_path,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Fetch {
    /// The [`RepositoryReference`] reference that was fetched.
    repository: RepositoryReference,

    /// The original input git url
    url: GitUrl,

    /// The precise git checkout
    commit: GitSha,

    /// The path to the checked-out repository.
    path: PathBuf,
}

impl Fetch {
    pub fn repository(&self) -> &RepositoryReference {
        &self.repository
    }

    pub fn input(&self) -> &GitUrl {
        &self.url
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
}

pub fn cache_digest(url: &RepositoryUrl) -> String {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    let hash = hasher.finish();
    format!("{:x}", hash)
}
