//! Integration tests for [`pixi_compute_sources::CheckoutUrl`] and the
//! [`pixi_compute_sources::UrlSourceCheckoutExt`] entry points.
//!
//! All tests use `file://` URLs so they're network-free. Test data
//! lives at `tests/data/url/hello_world.zip` (workspace root).

mod common;

use common::{
    EngineConfig, LifecycleReporter, MaxInFlightReporter, build_test_engine, dummy_sha,
    file_url_for_test, prepare_cached_checkout, test_tempdir, to_abs_dir,
};
use pixi_compute_cache_dirs::CacheDirs;
use pixi_compute_sources::{SourceCheckoutError, UrlSourceCheckoutExt};
use pixi_record::PinnedSourceSpec;
use pixi_spec::{Subdirectory, UrlSpec};
use pixi_url::UrlError;
use rattler_digest::{Sha256, digest::Digest};

/// A pre-staged checkout under the cache root is reused without any
/// download attempt: the resolver finds the marker file plus payload.
#[tokio::test]
async fn pin_and_checkout_url_reuses_cached_checkout() {
    let tempdir = test_tempdir();
    let cache_dirs = CacheDirs::new(to_abs_dir(tempdir.path().join("pixi-cache")));
    let url_cache_root = cache_dirs.resolve_from_env::<pixi_compute_sources::UrlDir>();

    let sha = dummy_sha();
    let checkout_dir = prepare_cached_checkout(url_cache_root.as_std_path(), sha);

    let engine = build_test_engine(EngineConfig {
        cache_dirs: Some(cache_dirs),
        sequential: true,
        ..Default::default()
    });

    let spec = UrlSpec {
        url: "https://example.com/archive.tar.gz".parse().unwrap(),
        md5: None,
        sha256: Some(sha),
        subdirectory: Subdirectory::default(),
    };

    let spec_for_engine = spec.clone();
    let checkout = engine
        .with_ctx(async |ctx| ctx.pin_and_checkout_url(spec_for_engine).await)
        .await
        .expect("engine scope should succeed")
        .expect("url checkout should succeed");

    assert_eq!(checkout.path.as_std_path(), checkout_dir);
    match checkout.pinned {
        PinnedSourceSpec::Url(pinned) => {
            assert_eq!(pinned.url, spec.url);
            assert_eq!(pinned.sha256, sha);
        }
        other => panic!("expected url pinned spec, got {other:?}"),
    }
}

/// Two concurrent requests for the same URL with mismatched expected
/// sha256 surface a `Sha256Mismatch` error on the bad request even
/// though the good request succeeds in parallel.
#[tokio::test]
async fn pin_and_checkout_url_reports_sha_mismatch_from_concurrent_request() {
    let tempdir = test_tempdir();
    let cache_dirs = CacheDirs::new(to_abs_dir(tempdir.path().join("pixi-cache")));
    let archive = test_tempdir();
    let url = file_url_for_test(&archive, "archive.zip");

    let engine = build_test_engine(EngineConfig {
        cache_dirs: Some(cache_dirs),
        ..Default::default()
    });

    let good_spec = UrlSpec {
        url: url.clone(),
        md5: None,
        sha256: None,
        subdirectory: Subdirectory::default(),
    };
    let bad_spec = UrlSpec {
        url,
        md5: None,
        sha256: Some(Sha256::digest(b"pixi-url-bad-sha")),
        subdirectory: Subdirectory::default(),
    };

    let (good, bad) = tokio::join!(
        engine.with_ctx(async |ctx| ctx.pin_and_checkout_url(good_spec).await),
        engine.with_ctx(async |ctx| ctx.pin_and_checkout_url(bad_spec).await),
    );

    assert!(good.expect("engine scope").is_ok());
    assert!(matches!(
        bad.expect("engine scope"),
        Err(SourceCheckoutError::UrlError(
            UrlError::Sha256Mismatch { .. }
        )),
    ));
}

/// A successful fetch followed by a fetch with a forced-bad sha256 on
/// the same URL must reject the cached entry rather than returning
/// the stale value.
#[tokio::test]
async fn pin_and_checkout_url_validates_cached_results() {
    let tempdir = test_tempdir();
    let cache_dirs = CacheDirs::new(to_abs_dir(tempdir.path().join("pixi-cache")));
    let archive = test_tempdir();
    let url = file_url_for_test(&archive, "archive.zip");

    let engine = build_test_engine(EngineConfig {
        cache_dirs: Some(cache_dirs),
        sequential: true,
        ..Default::default()
    });

    let spec = UrlSpec {
        url: url.clone(),
        md5: None,
        sha256: None,
        subdirectory: Subdirectory::default(),
    };

    engine
        .with_ctx(async |ctx| ctx.pin_and_checkout_url(spec).await)
        .await
        .expect("engine scope")
        .expect("initial download succeeds");

    let bad_spec = UrlSpec {
        url: url.clone(),
        md5: None,
        sha256: Some(Sha256::digest(b"pixi-url-bad-cache")),
        subdirectory: Subdirectory::default(),
    };

    let err = engine
        .with_ctx(async |ctx| ctx.pin_and_checkout_url(bad_spec).await)
        .await
        .expect("engine scope")
        .unwrap_err();
    assert!(matches!(
        err,
        SourceCheckoutError::UrlError(UrlError::Sha256Mismatch { .. })
    ));
}

/// One URL checkout fires the full reporter sequence. The
/// [`LifecycleReporter`] asserts ordering and exactly-once internally;
/// the test just asserts the run reached the terminal state.
#[tokio::test]
async fn url_checkout_fires_full_reporter_lifecycle() {
    let tempdir = test_tempdir();
    let cache_dirs = CacheDirs::new(to_abs_dir(tempdir.path().join("pixi-cache")));
    let archive = test_tempdir();
    let url = file_url_for_test(&archive, "archive.zip");

    let reporter = LifecycleReporter::new();
    let engine = build_test_engine(EngineConfig {
        cache_dirs: Some(cache_dirs),
        url_reporter: Some(reporter.clone()),
        sequential: true,
        ..Default::default()
    });

    engine
        .with_ctx(async |ctx| {
            ctx.pin_and_checkout_url(UrlSpec {
                url,
                md5: None,
                sha256: None,
                subdirectory: Subdirectory::default(),
            })
            .await
        })
        .await
        .expect("engine scope")
        .expect("url checkout should succeed");

    reporter.assert_complete();
}

/// With `max_concurrent_url_checkouts = 1` the URL semaphore must
/// prevent more than one checkout from being in flight simultaneously.
/// The custom reporter tracks max in-flight count.
#[tokio::test]
async fn url_checkout_semaphore_limits_inflight_count() {
    let tempdir = test_tempdir();
    let cache_dirs = CacheDirs::new(to_abs_dir(tempdir.path().join("pixi-cache")));
    let archive = test_tempdir();
    let url_a = file_url_for_test(&archive, "a.zip");
    let url_b = file_url_for_test(&archive, "b.zip");
    let url_c = file_url_for_test(&archive, "c.zip");

    let reporter = MaxInFlightReporter::new();
    let engine = build_test_engine(EngineConfig {
        cache_dirs: Some(cache_dirs),
        url_reporter: Some(reporter.clone()),
        max_concurrent_url: Some(1),
        ..Default::default()
    });

    let mk = |url: url::Url| UrlSpec {
        url,
        md5: None,
        sha256: None,
        subdirectory: Subdirectory::default(),
    };

    let (a, b, c) = tokio::join!(
        engine.with_ctx(async |ctx| ctx.pin_and_checkout_url(mk(url_a)).await),
        engine.with_ctx(async |ctx| ctx.pin_and_checkout_url(mk(url_b)).await),
        engine.with_ctx(async |ctx| ctx.pin_and_checkout_url(mk(url_c)).await),
    );
    a.expect("a engine scope").expect("a should succeed");
    b.expect("b engine scope").expect("b should succeed");
    c.expect("c engine scope").expect("c should succeed");

    assert_eq!(
        reporter.max_seen(),
        1,
        "semaphore should serialize URL checkouts"
    );
}
