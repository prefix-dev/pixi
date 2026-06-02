//! Integration tests for the Git LFS fetch path. Uses the `lfs-sample`
//! fixture (a tiny repo with `*.bin filter=lfs` in `.gitattributes` and
//! one binary file). Requires `git-lfs` on the host.

use std::path::Path;

use pixi_git::{GitUrl, sha::GitSha, source::GitSource};
use pixi_test_utils::GitRepoFixture;
use rattler_networking::LazyClient;
use reqwest_middleware::ClientWithMiddleware;

/// LazyClient that panics if HTTP is touched. file:// URLs never trigger it.
fn panic_client() -> LazyClient {
    LazyClient::new(|| -> ClientWithMiddleware {
        panic!("network should not be used in LFS tests")
    })
}

/// Returns whether `path` looks like a git-lfs pointer file.
fn is_lfs_pointer(path: &Path) -> bool {
    let contents = fs_err::read_to_string(path).unwrap_or_default();
    contents.starts_with("version https://git-lfs.github.com/spec/")
}

/// Skip the test when `git lfs version` doesn't work on this host.
fn require_git_lfs(test: &str) -> bool {
    let ok = std::process::Command::new("git")
        .args(["lfs", "version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        eprintln!("skipping {test}: git-lfs is not installed");
    }
    ok
}

/// The LFS fixture itself builds: detection fires, `git lfs install --local`
/// runs, and `data.bin` lands in the repo as an LFS pointer with the blob
/// present under `.git/lfs/objects/`.
#[test]
fn fixture_builds_with_lfs() {
    if !require_git_lfs("fixture_builds_with_lfs") {
        return;
    }
    let repo = GitRepoFixture::new("lfs-sample");
    assert!(repo.uses_lfs, "fixture should auto-detect LFS");

    // `data.bin` in the working tree is materialised (smudge ran during the
    // commit-then-checkout cycle), but the index entry is an LFS pointer.
    let pointer = repo.git(&["show", "HEAD:data.bin"]);
    assert!(
        pointer.starts_with("version https://git-lfs.github.com/spec/"),
        "HEAD:data.bin should be an LFS pointer, got: {pointer:?}"
    );

    // The actual blob is in the repo's LFS object store.
    let objects = repo.repo_path.join(".git/lfs/objects");
    assert!(
        objects.is_dir() && fs_err::read_dir(&objects).unwrap().next().is_some(),
        "expected LFS objects under {}",
        objects.display()
    );
}

/// Without `with_lfs(Some(true))`, `GitSource::fetch` honours
/// `GIT_LFS_SKIP_SMUDGE` defaults and leaves LFS pointers in the checkout.
#[test]
fn fetch_without_lfs_leaves_pointer() {
    if !require_git_lfs("fetch_without_lfs_leaves_pointer") {
        return;
    }
    let repo = GitRepoFixture::new("lfs-sample");
    let cache = tempfile::tempdir().unwrap();

    let git_url = GitUrl::try_from(repo.base_url.clone()).unwrap();
    // Tri-state `None` = "no opinion": don't set GIT_LFS_SKIP_SMUDGE, don't
    // run `git lfs fetch`. The smudge filter still runs during reset, but
    // git-lfs has nothing to fetch from a brand-new clone, so files end up
    // as the pointer.
    let fetch = GitSource::new(git_url, panic_client(), cache.path())
        .with_lfs(Some(false))
        .fetch()
        .expect("fetch should succeed");

    assert!(!*fetch.lfs_ready(), "LFS was not requested");
    let data = fetch.path().join("data.bin");
    assert!(data.is_file(), "data.bin missing from checkout");
    assert!(
        is_lfs_pointer(&data),
        "data.bin should still be a pointer when LFS is disabled"
    );
}

/// With `with_lfs(Some(true))`, `GitSource::fetch` runs `git lfs fetch`,
/// validates with `git lfs fsck`, and materialises pointer files into the
/// real blob content during the subsequent `git reset --hard`.
#[test]
fn fetch_with_lfs_materialises_blob() {
    if !require_git_lfs("fetch_with_lfs_materialises_blob") {
        return;
    }
    let repo = GitRepoFixture::new("lfs-sample");
    let original = fs_err::read(repo.repo_path.join("data.bin")).unwrap();
    let cache = tempfile::tempdir().unwrap();

    let git_url = GitUrl::try_from(repo.base_url.clone()).unwrap();
    let fetch = GitSource::new(git_url, panic_client(), cache.path())
        .with_lfs(Some(true))
        .fetch()
        .expect("fetch should succeed");

    assert!(
        *fetch.lfs_ready(),
        "fsck should pass for a healthy LFS fixture"
    );

    let data = fetch.path().join("data.bin");
    assert!(data.is_file());
    assert!(
        !is_lfs_pointer(&data),
        "data.bin should be the real blob, not a pointer"
    );
    let got = fs_err::read(&data).unwrap();
    assert_eq!(
        got, original,
        "checked-out data.bin should match fixture source"
    );
}

/// Second fetch against a warm cache (same `cache` dir, same precise rev)
/// hits the cached-DB branch in `GitSource::fetch`. With LFS requested, the
/// branch also requires `db.contains_lfs_artifacts(rev)`; it does after the
/// first fetch populated `.git/lfs/objects/`, so the second fetch returns
/// `lfs_ready == true` without touching the remote.
#[test]
fn cached_fetch_with_lfs_artifacts_is_ready() {
    if !require_git_lfs("cached_fetch_with_lfs_artifacts_is_ready") {
        return;
    }
    let repo = GitRepoFixture::new("lfs-sample");
    let cache = tempfile::tempdir().unwrap();
    let head: GitSha = repo.latest_commit().parse().unwrap();

    let make_source = || {
        let url = GitUrl::try_from(repo.base_url.clone())
            .unwrap()
            .with_precise(head);
        GitSource::new(url, panic_client(), cache.path()).with_lfs(Some(true))
    };

    // Warm the cache.
    let first = make_source().fetch().expect("first fetch should succeed");
    assert!(*first.lfs_ready());

    // Second fetch should reuse the DB and skip the network entirely while
    // still reporting LFS as ready.
    let second = make_source().fetch().expect("cached fetch should succeed");
    assert!(*second.lfs_ready());
    assert_eq!(second.commit(), first.commit());
}
