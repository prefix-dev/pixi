//! Tests for offline mode.
//!
//! The test harness runs every test in offline mode by default (see
//! `common::default_project_config`), so most of the suite already proves
//! that pixi works offline with local channels. The tests here cover the
//! error paths: operations that need data that is not available locally must
//! fail with an error that explains offline mode caused it.

use pixi_cli::offline::attach_offline_hint;
use pixi_consts::consts;
use pixi_test_utils::{MockRepoData, Package, format_diagnostic};
use rattler_conda_types::Platform;
use tempfile::TempDir;

use crate::common::{LockFileExt, PixiControl};
use crate::setup_tracing;

/// Render a report the way the CLI does: offline hint attached, then the
/// graphical diagnostic.
fn render_offline_report(report: miette::Report) -> String {
    let report = attach_offline_hint(report);
    format_diagnostic(report.as_ref())
}

/// Normalizers for machine-specific parts of rendered diagnostics.
fn offline_snapshot_filters() -> Vec<(&'static str, &'static str)> {
    vec![
        // The queried subdir depends on the platform running the tests, and
        // which subdir errors first is racy.
        (
            r"noarch|linux-[a-z0-9]+|osx-[a-z0-9]+|win-[a-z0-9]+",
            "<subdir>",
        ),
    ]
}

/// Solving and locking against a local `file://` channel works in offline
/// mode: local channels don't require network access.
#[tokio::test]
async fn offline_add_from_local_channel_succeeds() {
    setup_tracing();

    let mut package_database = MockRepoData::default();
    package_database.add_package(Package::build("foo", "1").finish());

    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    // Force offline mode explicitly so the test also holds when the
    // `online_tests` feature lifts the harness-level offline default.
    let pixi = PixiControl::new().unwrap().with_offline_mode();
    pixi.init()
        .with_local_channel(channel_dir.path())
        .await
        .unwrap();

    pixi.add("foo==1").await.unwrap();

    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "foo==1"
    ));
}

/// Searching a remote channel without cached repodata fails, and the rendered
/// error explains that offline mode caused it.
#[tokio::test]
async fn offline_search_without_cached_repodata_fails_with_hint() {
    setup_tracing();

    // Force offline mode explicitly so the test also holds when the
    // `online_tests` feature lifts the harness-level offline default.
    let pixi = PixiControl::new().unwrap().with_offline_mode();
    // `pixi init` defaults to the `https://prefix.dev/conda-forge` channel.
    pixi.init().await.unwrap();

    // Point the cache to an empty directory so a warm developer/CI cache
    // cannot satisfy the query.
    let cache_dir = TempDir::new().unwrap();
    let result = temp_env::async_with_vars(
        [("PIXI_CACHE_DIR", Some(cache_dir.path().as_os_str()))],
        async { pixi.search("doesnotexist".to_string()).await },
    )
    .await;

    let err = result.expect_err("searching a remote channel offline without a cache must fail");
    insta::with_settings!({filters => offline_snapshot_filters()}, {
        insta::assert_snapshot!(render_offline_report(err), @"
        × the sharded index cache for https://prefix.dev/conda-forge/<subdir>/ is not available
        help: pixi is running in offline mode and only uses locally cached data.
              Retry with network access: remove the `--offline` flag, unset the `PIXI_OFFLINE` environment variable, or disable the `offline` option in your pixi configuration.
        ");
    });
}

/// Solving an environment with a remote channel and no cached repodata fails,
/// and the rendered error explains that offline mode caused it.
#[tokio::test]
async fn offline_solve_without_cached_repodata_fails_with_hint() {
    setup_tracing();

    // Force offline mode explicitly so the test also holds when the
    // `online_tests` feature lifts the harness-level offline default.
    let pixi = PixiControl::new().unwrap().with_offline_mode();
    pixi.init().await.unwrap();

    let cache_dir = TempDir::new().unwrap();
    let result = temp_env::async_with_vars(
        [("PIXI_CACHE_DIR", Some(cache_dir.path().as_os_str()))],
        async { pixi.add("some-package").await },
    )
    .await;

    let err =
        result.expect_err("solving against a remote channel offline without a cache must fail");
    insta::with_settings!({filters => offline_snapshot_filters()}, {
        insta::assert_snapshot!(render_offline_report(err), @"
        × failed to solve requirements of environment 'default' for platform '<subdir>'
        ╰─▶   × the sharded index cache for https://prefix.dev/conda-forge/<subdir>/ is not available

        help: pixi is running in offline mode and only uses locally cached data.
              Retry with network access: remove the `--offline` flag, unset the `PIXI_OFFLINE` environment variable, or disable the `offline` option in your pixi configuration.
        ");
    });
}

/// The `--offline` flag alone (without the config option) puts the whole
/// stack in offline mode.
#[tokio::test]
async fn offline_flag_is_honored() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    let cache_dir = TempDir::new().unwrap();
    let result = temp_env::async_with_vars(
        [("PIXI_CACHE_DIR", Some(cache_dir.path().as_os_str()))],
        async {
            // `Some(true)` is exactly what a parsed `--offline` produces; no
            // project or global config sets `offline`, so the flag value is
            // what drives the whole stack.
            let mut search = pixi.search("doesnotexist".to_string());
            search.args.config.offline = Some(true);
            search.await
        },
    )
    .await;

    let err = result.expect_err("searching with --offline and an empty cache must fail");
    let rendered = render_offline_report(err);
    assert!(
        rendered.contains("pixi is running in offline mode"),
        "the rendered error should explain offline mode, got:\n{rendered}"
    );
}
