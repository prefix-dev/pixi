//! Integration tests for reference resolution in `GitSource::fetch`.
//! Uses the `minimal-pypi-package` fixture (two commits, tags `v0.1.0`
//! and `v0.2.0`).

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
        panic!("network should not be used in checkout tests")
    })
}

/// Fetch the fixture repository at the given `rev` string, as a manifest
/// `rev = "..."` entry would.
fn fetch_rev(repo: &GitRepoFixture, cache: &Path, rev: &str) -> Result<Fetch, GitError> {
    let git_url = GitUrl::try_from(repo.base_url.clone())
        .unwrap()
        .with_reference(GitReference::from_rev(rev.to_string()));
    GitSource::new(git_url, panic_client(), cache).fetch()
}

/// A `rev` that names a tag resolves to the tag's commit and produces a
/// populated checkout (#6589). `rev` maps to `BranchOrTag`, so the branch
/// candidate fails and resolution must fall through to the tag.
#[test]
fn rev_naming_a_tag_resolves_to_the_tag_commit() {
    let repo = GitRepoFixture::new("minimal-pypi-package");
    let cache = tempfile::tempdir().unwrap();

    let fetch = fetch_rev(&repo, cache.path(), "v0.1.0").expect("fetch should succeed");

    assert_eq!(fetch.commit().to_string(), repo.tag_commit("v0.1.0"));
    assert!(
        fetch.path().join("pyproject.toml").is_file(),
        "checkout at {} should contain the fixture files",
        fetch.path().display()
    );
}

/// A hash-looking `rev` that exists nowhere in the repository fails with an
/// error naming the rev, instead of returning a broken empty checkout.
#[test]
fn rev_that_does_not_exist_errors() {
    let repo = GitRepoFixture::new("minimal-pypi-package");
    let cache = tempfile::tempdir().unwrap();

    // Hash-like, so the fetch itself succeeds (all branches and tags) and
    // only the resolution step can report the failure.
    let err = fetch_rev(&repo, cache.path(), "deadbeef").expect_err("fetch should fail");

    assert!(
        err.to_string().contains("deadbeef"),
        "error should name the unresolved rev, got: {err}"
    );
}
