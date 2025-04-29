/// Derived from `uv-git` implementation
/// Source: https://github.com/astral-sh/uv/blob/4b8cc3e29e4c2a6417479135beaa9783b05195d3/crates/uv-git/src/resolver.rs
/// This module expose types and functions to interact with Git repositories.
use std::path::PathBuf;
use std::sync::Arc;

use pixi_utils::AsyncPrefixGuard;
use tracing::debug;

use dashmap::DashMap;
use dashmap::mapref::one::Ref;
use reqwest_middleware::ClientWithMiddleware;

use crate::{
    GitError, GitUrl, Reporter,
    git::GitReference,
    sha::GitSha,
    source::{Fetch, GitSource, cache_digest},
    url::RepositoryUrl,
};

#[derive(Debug, thiserror::Error)]
pub enum GitResolverError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
    #[error("Git operation failed")]
    Git(String),
}

/// [`GitResolver`] is responsible for managing and resolving Git repository references.
/// It maintains a mapping between [`RepositoryReference`] (e.g., a Git URL with branch or tag) and the precise commit [`GitSha`]
/// and it also provides methods to fetch Git repositories from given [`GitUrl`] and cache it.
#[derive(Default, Clone)]
pub struct GitResolver(Arc<DashMap<RepositoryReference, GitSha>>);

impl GitResolver {
    /// Inserts a new [`GitSha`] for the given [`RepositoryReference`].
    pub fn insert(&self, reference: RepositoryReference, sha: GitSha) {
        self.0.insert(reference, sha);
    }

    /// Returns the [`GitSha`] for the given [`RepositoryReference`], if it exists.
    fn get(&self, reference: &RepositoryReference) -> Option<Ref<RepositoryReference, GitSha>> {
        self.0.get(reference)
    }

    /// Fetch a remote Git repository.
    pub async fn fetch(
        &self,
        url: GitUrl,
        client: ClientWithMiddleware,
        cache: PathBuf,
        reporter: Option<Arc<dyn Reporter>>,
    ) -> Result<Fetch, GitError> {
        debug!("Fetching source distribution from Git: {url}");

        let reference = RepositoryReference::from(&url);

        // If we know the precise commit already, reuse it, to ensure that all fetches within a
        // single process are consistent.
        let url = {
            if let Some(precise) = self.get(&reference) {
                url.with_precise(*precise)
            } else {
                url
            }
        };

        // Avoid races between different processes, too.
        let lock_dir = cache.join("locks");
        let repository_url = RepositoryUrl::new(url.repository());

        let write_guard_path = lock_dir.join(cache_digest(&repository_url));
        let guard = AsyncPrefixGuard::new(&write_guard_path).await?;
        let mut write_guard = guard.write().await?;

        // Update the prefix to indicate that we are installing it.
        write_guard.begin().await?;

        // Fetch the Git repository.
        let source = GitSource::new(url.clone(), client, cache);
        let source = if let Some(reporter) = reporter {
            source.with_reporter(reporter)
        } else {
            source
        };

        let fetch = tokio::task::spawn_blocking(move || source.fetch())
            .await?
            .inspect_err(|err| tracing::error!("Error fetching Git repository: {err}"))?;

        // Insert the resolved URL into the in-memory cache. This ensures that subsequent fetches
        // resolve to the same precise commit.
        self.insert(reference, fetch.commit());

        write_guard.finish().await?;

        tracing::debug!("Fetched source distribution from Git: {url}");
        Ok(fetch)
    }

    /// Given a remote source distribution, return a precise variant, if possible.
    ///
    /// For example, given a Git dependency with a reference to a branch or tag, return a URL
    /// with a precise reference to the current commit of that branch or tag.
    ///
    /// This method takes into account various normalizations that are independent from the Git
    /// layer. For example: removing `#subdirectory=pkg_dir`-like fragments, and removing `git+`
    /// prefix kinds.
    ///
    /// This method will only return precise URLs for URLs that have already been resolved via
    /// `resolve_precise`, and will return `None` for URLs that have not been resolved _or_
    /// already have a precise reference.
    pub fn precise(&self, url: GitUrl) -> Option<GitUrl> {
        let reference = RepositoryReference::from(&url);
        let precise = self.get(&reference)?;
        Some(url.with_precise(*precise))
    }

    /// Returns `true` if the two Git URLs refer to the same precise commit.
    pub fn same_ref(&self, a: &GitUrl, b: &GitUrl) -> bool {
        // Convert `a` to a repository URL.
        let a_ref = RepositoryReference::from(a);

        // Convert `b` to a repository URL.
        let b_ref = RepositoryReference::from(b);

        // The URLs must refer to the same repository.
        if a_ref.url != b_ref.url {
            return false;
        }

        // If the URLs have the same tag, they refer to the same commit.
        if a_ref.reference == b_ref.reference {
            return true;
        }

        // Otherwise, the URLs must resolve to the same precise commit.
        let Some(a_precise) = a.precise().or_else(|| self.get(&a_ref).map(|sha| *sha)) else {
            return false;
        };

        let Some(b_precise) = b.precise().or_else(|| self.get(&b_ref).map(|sha| *sha)) else {
            return false;
        };

        a_precise == b_precise
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedRepositoryReference {
    /// An abstract reference to a Git repository, including the URL and the commit (e.g., a branch,
    /// tag, or revision).
    pub reference: RepositoryReference,
    /// The precise commit SHA of the reference.
    pub sha: GitSha,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepositoryReference {
    /// The URL of the Git repository, with any query parameters and fragments removed.
    pub url: RepositoryUrl,
    /// The reference to the commit to use, which could be a branch, tag, or revision.
    pub reference: GitReference,
}

impl From<&GitUrl> for RepositoryReference {
    fn from(git: &GitUrl) -> Self {
        Self {
            url: RepositoryUrl::new(git.repository()),
            reference: git.reference().clone(),
        }
    }
}
