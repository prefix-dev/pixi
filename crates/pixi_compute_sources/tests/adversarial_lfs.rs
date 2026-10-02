//! Adversarial integration tests for the `lfs` flag on git source
//! checkouts. They exercise cache interactions between LFS and plain
//! checkouts of the same commit, the lock file round trip, explicit
//! `lfs = false`, broken LFS remotes, and repos without LFS files.
//!
//! Tests use [`pixi_test_utils::GitRepoFixture`] (local `git` binary,
//! no network) and require `git-lfs` on the host; they skip otherwise.

mod common;

use std::path::Path;

use common::{EngineConfig, build_test_engine};
use pixi_compute_sources::{GitSourceCheckoutExt, SourceCheckoutError};
use pixi_record::{PinnedGitSpec, PinnedSourceSpec};
use pixi_spec::{GitLocationSpec, Subdirectory};
use pixi_test_utils::{GitRepoFixture, git_lfs_available};

/// Returns whether `path` looks like a git-lfs pointer file.
fn is_lfs_pointer(path: &Path) -> bool {
    let contents = fs_err::read_to_string(path).unwrap_or_default();
    contents.starts_with("version https://git-lfs.github.com/spec/")
}

/// Skip helper mirroring the one in `pixi_git/tests/lfs.rs`.
fn require_git_lfs(test: &str) -> bool {
    if !git_lfs_available() {
        eprintln!("skipping {test}: git-lfs is not installed");
        return false;
    }
    true
}

/// Extracts the pinned git spec from a checkout or panics.
fn pinned_git(pinned: &PinnedSourceSpec) -> &PinnedGitSpec {
    match pinned {
        PinnedSourceSpec::Git(git) => git,
        other => panic!("expected git pinned spec, got {other:?}"),
    }
}

/// Checking out the same commit twice in one process, first plain
/// (`lfs = None`) and then with `lfs = Some(true)`, yields two distinct
/// checkout directories and the LFS checkout has the real blob. The
/// plain checkout must not be reused for (or poisoned by) the LFS one.
#[tokio::test]
async fn same_commit_plain_then_lfs() {
    if !require_git_lfs("same_commit_plain_then_lfs") {
        return;
    }
    let repo = GitRepoFixture::new("lfs-sample");
    let original = fs_err::read(repo.repo_path.join("data.bin")).expect("fixture has data.bin");

    let engine = build_test_engine(EngineConfig {
        sequential: true,
        ..Default::default()
    });

    let base_spec = GitLocationSpec::new(repo.base_url.clone(), None, Subdirectory::default());
    let plain_spec = base_spec.clone();
    let lfs_spec = base_spec.with_lfs(Some(true));

    let (plain, lfs) = engine
        .with_ctx(async |ctx| {
            let plain = ctx.pin_and_checkout_git(plain_spec).await?;
            let lfs = ctx.pin_and_checkout_git(lfs_spec).await?;
            Ok::<_, SourceCheckoutError>((plain, lfs))
        })
        .await
        .expect("engine scope")
        .expect("both checkouts should succeed");

    assert_ne!(
        plain.path, lfs.path,
        "plain and LFS checkouts of the same commit must live in different directories"
    );
    assert_eq!(pinned_git(&plain.pinned).source.lfs, None);
    assert_eq!(pinned_git(&lfs.pinned).source.lfs, Some(true));
    assert_eq!(
        pinned_git(&plain.pinned).source.commit,
        pinned_git(&lfs.pinned).source.commit,
        "both checkouts should pin the same commit"
    );

    let lfs_data = lfs.path.as_std_path().join("data.bin");
    assert!(
        !is_lfs_pointer(&lfs_data),
        "LFS checkout must contain the real blob, not a pointer"
    );
    assert_eq!(
        fs_err::read(&lfs_data).expect("data.bin in LFS checkout"),
        original,
        "LFS checkout content must match the fixture source"
    );
}

/// The reverse order: `lfs = Some(true)` first, then `lfs = None`. The
/// second (plain) checkout must not silently reuse the LFS checkout
/// directory, and the LFS checkout stays intact afterwards.
#[tokio::test]
async fn same_commit_lfs_then_plain() {
    if !require_git_lfs("same_commit_lfs_then_plain") {
        return;
    }
    let repo = GitRepoFixture::new("lfs-sample");
    let original = fs_err::read(repo.repo_path.join("data.bin")).expect("fixture has data.bin");

    let engine = build_test_engine(EngineConfig {
        sequential: true,
        ..Default::default()
    });

    let base_spec = GitLocationSpec::new(repo.base_url.clone(), None, Subdirectory::default());
    let lfs_spec = base_spec.clone().with_lfs(Some(true));
    let plain_spec = base_spec;

    let (lfs, plain) = engine
        .with_ctx(async |ctx| {
            let lfs = ctx.pin_and_checkout_git(lfs_spec).await?;
            let plain = ctx.pin_and_checkout_git(plain_spec).await?;
            Ok::<_, SourceCheckoutError>((lfs, plain))
        })
        .await
        .expect("engine scope")
        .expect("both checkouts should succeed");

    assert_ne!(
        lfs.path, plain.path,
        "LFS and plain checkouts of the same commit must live in different directories"
    );
    assert_eq!(pinned_git(&lfs.pinned).source.lfs, Some(true));
    assert_eq!(pinned_git(&plain.pinned).source.lfs, None);

    let lfs_data = lfs.path.as_std_path().join("data.bin");
    assert!(
        !is_lfs_pointer(&lfs_data),
        "LFS checkout must still contain the real blob after the plain checkout ran"
    );
    assert_eq!(
        fs_err::read(&lfs_data).expect("data.bin in LFS checkout"),
        original,
    );
}

/// `lfs = Some(false)` succeeds, leaves the pointer file in place, and
/// records the preference in the pinned spec.
#[tokio::test]
async fn explicit_lfs_false_keeps_pointer() {
    if !require_git_lfs("explicit_lfs_false_keeps_pointer") {
        return;
    }
    let repo = GitRepoFixture::new("lfs-sample");

    let engine = build_test_engine(EngineConfig {
        sequential: true,
        ..Default::default()
    });

    let spec = GitLocationSpec::new(repo.base_url.clone(), None, Subdirectory::default())
        .with_lfs(Some(false));

    let checkout = engine
        .with_ctx(async |ctx| ctx.pin_and_checkout_git(spec).await)
        .await
        .expect("engine scope")
        .expect("checkout with lfs = false should succeed");

    assert_eq!(pinned_git(&checkout.pinned).source.lfs, Some(false));
    let data = checkout.path.as_std_path().join("data.bin");
    assert!(data.is_file(), "data.bin missing from checkout");
    assert!(
        is_lfs_pointer(&data),
        "data.bin must remain an LFS pointer when lfs = false"
    );
}

/// Full lock round trip with a subdirectory: pin a checkout with
/// `lfs = Some(true)` and `subdirectory = "pkg"`, serialize the pin to a
/// locked git URL, parse it back, and check out the parsed pin in a
/// completely fresh engine with a fresh cache. The re-checkout must
/// materialize the real LFS content inside the subdirectory.
#[tokio::test]
async fn lock_roundtrip_with_subdirectory_materializes_lfs() {
    if !require_git_lfs("lock_roundtrip_with_subdirectory_materializes_lfs") {
        return;
    }

    // Build a custom fixture with the LFS file inside a subdirectory.
    let payload: Vec<u8> = (0u16..1024).map(|i| (i % 251) as u8).collect();
    let fixture_dir = tempfile::tempdir().expect("create fixture tempdir");
    let commit_dir = fixture_dir.path().join("001_v0.1.0");
    fs_err::create_dir_all(commit_dir.join("pkg")).unwrap();
    fs_err::write(
        commit_dir.join("dot-gitattributes"),
        "*.bin filter=lfs diff=lfs merge=lfs -text\n",
    )
    .unwrap();
    fs_err::write(commit_dir.join("pkg").join("data.bin"), &payload).unwrap();
    fs_err::write(commit_dir.join("README.md"), "lfs subdir fixture\n").unwrap();
    let repo = GitRepoFixture::from_path(fixture_dir.path(), "lfs-subdir");
    assert!(repo.uses_lfs, "fixture should auto-detect LFS");

    // First engine: resolve and pin.
    let engine = build_test_engine(EngineConfig {
        sequential: true,
        ..Default::default()
    });
    let spec = GitLocationSpec::new(
        repo.base_url.clone(),
        None,
        Subdirectory::try_from("pkg").unwrap(),
    )
    .with_lfs(Some(true));

    let checkout = engine
        .with_ctx(async |ctx| ctx.pin_and_checkout_git(spec).await)
        .await
        .expect("engine scope")
        .expect("initial checkout should succeed");
    let pinned = pinned_git(&checkout.pinned).clone();
    assert_eq!(pinned.source.lfs, Some(true));

    // Lock round trip: the URL carries both query pairs and parses back
    // into an identical pin.
    let locked = pinned.clone().into_locked_git_url();
    let locked_url = locked.to_url();
    assert!(
        locked_url
            .query_pairs()
            .any(|(k, v)| k == "lfs" && v == "true"),
        "locked URL should carry ?lfs=true: {locked_url}"
    );
    assert!(
        locked_url
            .query_pairs()
            .any(|(k, v)| k == "subdirectory" && v == "pkg"),
        "locked URL should carry ?subdirectory=pkg: {locked_url}"
    );
    let reparsed = locked.to_pinned_git_spec().expect("parse locked URL");
    assert_eq!(reparsed, pinned, "pin must survive the lock round trip");

    // Second engine with a fresh cache: check out from the parsed pin
    // only, as an install from the lock file would.
    let fresh_engine = build_test_engine(EngineConfig {
        sequential: true,
        ..Default::default()
    });
    let reinstalled = fresh_engine
        .with_ctx(async |ctx| ctx.checkout_pinned_git(reparsed).await)
        .await
        .expect("engine scope")
        .expect("pinned checkout should succeed");

    assert!(
        reinstalled.path.as_std_path().ends_with("pkg"),
        "checkout path should point into the subdirectory: {:?}",
        reinstalled.path
    );
    let data = reinstalled.path.as_std_path().join("data.bin");
    assert!(
        !is_lfs_pointer(&data),
        "pinned re-checkout must materialize the real blob"
    );
    assert_eq!(
        fs_err::read(&data).expect("data.bin in pinned checkout"),
        payload,
        "pinned re-checkout content must match the fixture source"
    );
}

/// A remote whose LFS objects are gone must fail the checkout when
/// `lfs = true` instead of silently leaving pointer files.
#[tokio::test]
async fn missing_remote_lfs_objects_error() {
    if !require_git_lfs("missing_remote_lfs_objects_error") {
        return;
    }
    let repo = GitRepoFixture::new("lfs-sample");

    // Wipe the LFS object store of the remote so `git lfs fetch` cannot
    // retrieve the blob.
    let objects = repo.repo_path.join(".git/lfs/objects");
    assert!(objects.is_dir(), "fixture should have LFS objects");
    fs_err::remove_dir_all(&objects).unwrap();

    let engine = build_test_engine(EngineConfig {
        sequential: true,
        ..Default::default()
    });

    let spec = GitLocationSpec::new(repo.base_url.clone(), None, Subdirectory::default())
        .with_lfs(Some(true));

    let result = engine
        .with_ctx(async |ctx| ctx.pin_and_checkout_git(spec).await)
        .await
        .expect("engine scope");

    // With git-lfs installed the checkout fails during `git reset --hard`
    // because `filter.lfs.required=true` makes the smudge failure fatal
    // (a `GitError`); without a usable smudge filter the fetch succeeds
    // with pointers and the checkout layer reports `LfsNotReady`. Both
    // are LFS-related errors; a silent pointer checkout is the bug.
    let err = result.expect_err("checkout must fail when LFS objects are missing from the remote");
    assert!(
        matches!(
            err,
            SourceCheckoutError::GitError(_) | SourceCheckoutError::LfsNotReady(_)
        ),
        "unexpected error variant: {err:?}"
    );
    assert!(
        err.to_string().to_lowercase().contains("lfs"),
        "error should mention LFS: {err}"
    );
}

/// In offline mode with a cold cache the LFS fetch is skipped, the git
/// checkout itself succeeds (local file transport) with pointer files,
/// and the checkout layer must then report [`SourceCheckoutError::LfsNotReady`]
/// instead of silently returning the pointers.
#[tokio::test]
async fn offline_cold_cache_lfs_true_reports_lfs_not_ready() {
    if !require_git_lfs("offline_cold_cache_lfs_true_reports_lfs_not_ready") {
        return;
    }
    let repo = GitRepoFixture::new("lfs-sample");

    let engine = build_test_engine(EngineConfig {
        sequential: true,
        offline: true,
        ..Default::default()
    });

    let spec = GitLocationSpec::new(repo.base_url.clone(), None, Subdirectory::default())
        .with_lfs(Some(true));

    let result = engine
        .with_ctx(async |ctx| ctx.pin_and_checkout_git(spec).await)
        .await
        .expect("engine scope");

    match result {
        Err(SourceCheckoutError::LfsNotReady(url)) => {
            assert!(
                url.contains("lfs-sample"),
                "error should name the repository, got: {url}"
            );
        }
        Err(other) => panic!("expected LfsNotReady, got: {other:?}"),
        Ok(checkout) => panic!(
            "expected LfsNotReady, got a successful checkout at {:?}",
            checkout.path
        ),
    }
}

/// `lfs = Some(true)` on a repository that has no LFS-tracked files at
/// all must succeed rather than error.
#[tokio::test]
async fn lfs_true_without_lfs_files_succeeds() {
    if !require_git_lfs("lfs_true_without_lfs_files_succeeds") {
        return;
    }
    let repo = GitRepoFixture::new("minimal-pypi-package");
    assert!(!repo.uses_lfs, "fixture must not use LFS");

    let engine = build_test_engine(EngineConfig {
        sequential: true,
        ..Default::default()
    });

    let spec = GitLocationSpec::new(repo.base_url.clone(), None, Subdirectory::default())
        .with_lfs(Some(true));

    let checkout = engine
        .with_ctx(async |ctx| ctx.pin_and_checkout_git(spec).await)
        .await
        .expect("engine scope")
        .expect("lfs = true on a repo without LFS files should succeed");

    assert_eq!(pinned_git(&checkout.pinned).source.lfs, Some(true));
    assert!(
        checkout.path.as_std_path().join("pyproject.toml").is_file(),
        "checkout should contain the repo contents"
    );
}
