//! Tests for offline mode in `GitSource::fetch`: network transports are
//! refused while local `file://` remotes and already-cached revisions keep
//! working.

use std::path::Path;

use pixi_git::{
    GitError, GitUrl,
    git::GitReference,
    source::{Fetch, GitSource},
};
use pixi_test_utils::GitRepoFixture;
use rattler_networking::LazyClient;
use reqwest_middleware::ClientWithMiddleware;

/// LazyClient that panics if HTTP is touched. file:// URLs never trigger it.
fn panic_client() -> LazyClient {
    LazyClient::new(|| -> ClientWithMiddleware {
        panic!("network should not be used in offline tests")
    })
}

/// Fetch the fixture repository at the given `rev` string in offline mode.
fn fetch_rev_offline(repo: &GitRepoFixture, cache: &Path, rev: &str) -> Result<Fetch, GitError> {
    let git_url = GitUrl::try_from(repo.base_url.clone())
        .unwrap()
        .with_reference(GitReference::from_rev(rev.to_string()));
    GitSource::new(git_url, panic_client(), cache)
        .with_offline(true)
        .fetch()
}

/// Local `file://` remotes are still allowed in offline mode; only network
/// transports are blocked.
#[test]
fn offline_fetch_from_local_file_remote_succeeds() {
    let repo = GitRepoFixture::new("minimal-pypi-package");
    let cache = tempfile::tempdir().unwrap();

    let fetch =
        fetch_rev_offline(&repo, cache.path(), "v0.1.0").expect("local fetch should succeed");

    assert_eq!(fetch.commit().to_string(), repo.tag_commit("v0.1.0"));

    // A repository without LFS files is fully materialized even though the
    // LFS fetch was skipped: the checkout must NOT be marked degraded, so
    // later online runs keep reusing it.
    let marker = fs_err::read_to_string(fetch.path().join(".ok"))
        .expect("checkout should have a ready marker");
    assert_eq!(marker, "", "plain repos must not be marked LFS-degraded");
}

/// Fetching over a network transport fails with `GitError::Offline` before
/// any connection is attempted (`GIT_ALLOW_PROTOCOL=file` makes git itself
/// refuse the transport).
#[test]
fn offline_fetch_from_network_remote_errors() {
    let cache = tempfile::tempdir().unwrap();

    let git_url = GitUrl::try_from(url::Url::parse("https://example.invalid/repo.git").unwrap())
        .unwrap()
        .with_reference(GitReference::DefaultBranch);
    let err = GitSource::new(git_url, panic_client(), cache.path())
        .with_offline(true)
        .fetch()
        .expect_err("fetching over https must fail in offline mode");

    assert!(matches!(err, GitError::Offline { .. }));
    insta::assert_snapshot!(
        err.to_string(),
        @"fetching git repository `https://example.invalid/repo.git` requires network access, but pixi is in offline mode and the requested revision is not available in the local cache"
    );
}

/// In offline mode the GitHub fast path must be skipped entirely: it is a
/// network optimization that queries the GitHub API, and offline safety for
/// github deps must come from the explicit `offline` flag rather than relying
/// on the client carrying `OfflineMiddleware`. `panic_client` panics if HTTP is
/// touched, so reaching `GitError::Offline` (git refusing the transport) proves
/// the fast path never consulted the client.
#[test]
fn offline_github_url_does_not_touch_http_client() {
    let cache = tempfile::tempdir().unwrap();
    let git_url =
        GitUrl::try_from(url::Url::parse("https://github.com/octocat/Hello-World.git").unwrap())
            .unwrap()
            .with_reference(GitReference::DefaultBranch);
    let err = GitSource::new(git_url, panic_client(), cache.path())
        .with_offline(true)
        .fetch()
        .expect_err("offline github fetch must fail");
    assert!(
        matches!(err, GitError::Offline { .. }),
        "expected Offline, got {err:?}"
    );
}
