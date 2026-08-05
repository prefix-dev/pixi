//! Regression tests for git-LFS handling in offline mode.
//!
//! Offline, the LFS smudge filter may only run when the local database holds
//! validated LFS artifacts — and then only against that database, never
//! against an endpoint from a committed `.lfsconfig`. Checkouts created with
//! a forcibly skipped smudge filter are marked LFS-degraded and re-created
//! once a fully materialized checkout is possible.

use std::path::Path;

use pixi_git::{GitUrl, sha::GitSha, source::GitSource};
use pixi_test_utils::GitRepoFixture;
use rattler_networking::LazyClient;
use reqwest_middleware::ClientWithMiddleware;

/// LazyClient that panics if HTTP is touched. file:// URLs never trigger it.
fn panic_client() -> LazyClient {
    LazyClient::new(|| -> ClientWithMiddleware {
        panic!("network should not be used in offline LFS tests")
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

/// The ready-marker contents of a checkout.
fn ready_marker(checkout: &Path) -> String {
    fs_err::read_to_string(checkout.join(".ok")).expect("checkout should have a ready marker")
}

/// An offline checkout without cached LFS objects degrades to pointer files
/// (marked as such), is reused while offline, and is re-created with the real
/// content on the next online fetch.
#[test]
fn offline_lfs_checkout_degrades_and_heals_online() {
    if !require_git_lfs("offline_lfs_checkout_degrades_and_heals_online") {
        return;
    }
    let repo = GitRepoFixture::new("lfs-sample");
    let original = fs_err::read(repo.repo_path.join("data.bin")).unwrap();
    let cache = tempfile::tempdir().unwrap();
    let head: GitSha = repo.latest_commit().parse().unwrap();
    let make_source = |offline| {
        let url = GitUrl::try_from(repo.base_url.clone())
            .unwrap()
            .with_precise(head);
        GitSource::new(url, panic_client(), cache.path())
            .with_lfs(Some(true))
            .with_offline(offline)
    };

    // 1. Offline with an empty cache: the repository itself is fetched (the
    //    fixture is a local `file://` remote) but the LFS fetch is skipped, so
    //    the checkout contains a pointer file and is marked degraded.
    let degraded = make_source(true).fetch().expect("offline fetch works");
    assert!(!*degraded.lfs_ready());
    assert!(is_lfs_pointer(&degraded.path().join("data.bin")));
    assert_eq!(ready_marker(degraded.path()), "lfs-degraded");

    // 2. A second offline fetch reuses the degraded checkout instead of
    //    re-creating it.
    let reused = make_source(true).fetch().expect("offline re-fetch works");
    assert_eq!(reused.path(), degraded.path());
    assert_eq!(ready_marker(reused.path()), "lfs-degraded");

    // 3. Back online the degraded checkout is not considered fresh: the LFS
    //    objects are fetched and the checkout is re-created with the real
    //    content and a clean marker.
    let healed = make_source(false).fetch().expect("online fetch works");
    assert!(*healed.lfs_ready());
    let data = healed.path().join("data.bin");
    assert!(!is_lfs_pointer(&data), "content should have materialized");
    assert_eq!(fs_err::read(&data).unwrap(), original);
    assert_eq!(ready_marker(healed.path()), "");
}

/// When the local database holds validated LFS artifacts, an offline checkout
/// materializes the content from that database — even when the repository
/// commits an `.lfsconfig` that points git-lfs at a (remote) server.
#[test]
fn offline_smudge_ignores_committed_lfsconfig() {
    if !require_git_lfs("offline_smudge_ignores_committed_lfsconfig") {
        return;
    }
    let repo = GitRepoFixture::new("lfs-sample");
    let original = fs_err::read(repo.repo_path.join("data.bin")).unwrap();
    let cache = tempfile::tempdir().unwrap();

    // 1. Warm the database (repository + LFS objects) from the clean fixture.
    let warm_url = GitUrl::try_from(repo.base_url.clone()).unwrap();
    let warm = GitSource::new(warm_url, panic_client(), cache.path())
        .with_lfs(Some(true))
        .fetch()
        .expect("warming fetch works");
    assert!(*warm.lfs_ready());

    // 2. Commit an `.lfsconfig` that redirects git-lfs to an unreachable
    //    server. If the offline smudge filter consulted it, the checkout
    //    below would fail (or, with a real server, silently use the network).
    fs_err::write(
        repo.repo_path.join(".lfsconfig"),
        "[lfs]\n\turl = https://lfs.example.invalid/api\n",
    )
    .unwrap();
    repo.git(&["add", ".lfsconfig"]);
    repo.git(&["commit", "-m", "add lfsconfig"]);
    let head: GitSha = repo.git(&["rev-parse", "HEAD"]).parse().unwrap();
    let make_source = |offline| {
        let url = GitUrl::try_from(repo.base_url.clone())
            .unwrap()
            .with_precise(head);
        GitSource::new(url, panic_client(), cache.path())
            .with_lfs(Some(true))
            .with_offline(offline)
    };

    // 3. Fetch the new revision offline: the repository update comes from the
    //    local fixture, the LFS fetch is skipped (degraded checkout), but the
    //    database now contains the new revision — and its LFS objects are
    //    still the validated ones from the warm-up.
    let degraded = make_source(true).fetch().expect("offline fetch works");
    assert_eq!(ready_marker(degraded.path()), "lfs-degraded");

    // 4. Drop the checkouts (keep the database) and fetch offline again: the
    //    cache-hit path allows the smudge filter, which must be pinned to the
    //    local database and ignore the committed `.lfsconfig`.
    fs_err::remove_dir_all(cache.path().join("checkouts")).unwrap();
    let offline_fetch = make_source(true)
        .fetch()
        .expect("offline checkout from a warm cache works despite .lfsconfig");
    assert!(*offline_fetch.lfs_ready());
    let data = offline_fetch.path().join("data.bin");
    assert!(
        !is_lfs_pointer(&data),
        "content should materialize from the local database"
    );
    assert_eq!(fs_err::read(&data).unwrap(), original);
    assert_eq!(ready_marker(offline_fetch.path()), "");
}
