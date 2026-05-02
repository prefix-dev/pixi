mod event_reporter;
mod event_tree;

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use pixi_path::AbsPathBuf;

use event_reporter::EventReporter;
use fs_err as fs;
use itertools::Itertools;
use pixi_build_backend_passthrough::{BackendEvent, ObservableBackend, PassthroughBackend};
use pixi_build_frontend::{BackendOverride, InMemoryOverriddenBackends};
use pixi_command_dispatcher::{
    CacheDirs, CommandDispatcher, CommandDispatcherError, EnvironmentRef, EnvironmentSpec,
    EphemeralEnv, Executor, InstallPixiEnvironmentExt, InstallPixiEnvironmentSpec,
    InstantiateToolEnvironmentSpec, SolvePixiEnvironmentError, SourceCheckoutError,
    keys::SolvePixiEnvironmentSpec, source_checkout::UrlSourceCheckoutExt,
};
use pixi_compute_engine::BuildEnvironment;
use pixi_record::PinnedSourceSpec;
use pixi_spec::{
    GitReference, GitSpec, PathSpec, PixiSpec, ResolvedExcludeNewer, Subdirectory, UrlSpec,
};
use pixi_spec_containers::DependencyMap;
use pixi_test_utils::format_diagnostic;
use pixi_url::UrlError;
use pixi_utils::variants::VariantConfig;
use rattler_conda_types::{
    ChannelUrl, GenericVirtualPackage, PackageName, Platform, VersionSpec, prefix::Prefix,
};
use rattler_digest::{Sha256, Sha256Hash, digest::Digest};
use rattler_virtual_packages::{VirtualPackageOverrides, VirtualPackages};
use tempfile::TempDir;
use url::Url;

use crate::{event_reporter::Event, event_tree::EventTree};
use pixi_command_dispatcher::{ReporterContextSpawnHook, source_checkout::UrlCheckoutSemaphore};
use pixi_compute_engine::ComputeEngine;
use tokio::sync::Semaphore;

/// Converts a PathBuf to AbsPresumedDirPathBuf for tests.
fn to_abs_dir(path: impl Into<PathBuf>) -> pixi_path::AbsPresumedDirPathBuf {
    AbsPathBuf::new(path)
        .expect("path is not absolute")
        .into_assume_dir()
}

/// Empty `SolvePixiEnvironmentSpec` for tests that only care about a
/// few fields. Used with struct update syntax:
/// `SolvePixiEnvironmentSpec { dependencies: ..., env_ref: env_ref_of(...),
/// ..empty_pixi_env_spec() }`.
/// Build matching `(installed, installed_source_hints)` from a `Vec`
/// of records, the way a real caller does.
fn installed_with_hints(
    installed: Vec<pixi_record::UnresolvedPixiRecord>,
) -> (
    std::sync::Arc<[pixi_record::UnresolvedPixiRecord]>,
    pixi_command_dispatcher::PtrArc<pixi_command_dispatcher::InstalledSourceHints>,
) {
    let installed: std::sync::Arc<[_]> = std::sync::Arc::from(installed);
    let hints = pixi_command_dispatcher::PtrArc::from_value(
        pixi_command_dispatcher::InstalledSourceHints::from_records(&installed),
    );
    (installed, hints)
}

fn empty_pixi_env_spec() -> SolvePixiEnvironmentSpec {
    SolvePixiEnvironmentSpec {
        dependencies: DependencyMap::default(),
        constraints: DependencyMap::default(),
        dev_sources: ordermap::OrderMap::new(),
        installed: std::sync::Arc::from([]),
        installed_source_hints: Default::default(),
        strategy: Default::default(),
        preferred_build_source: std::sync::Arc::new(BTreeMap::new()),
        env_ref: EnvironmentRef::Ephemeral(EphemeralEnv::new(
            "test",
            EnvironmentSpec {
                channels: Vec::new(),
                build_environment: BuildEnvironment::default(),
                variants: VariantConfig::default(),
                exclude_newer: None,
                channel_priority: Default::default(),
            },
        )),
    }
}

/// Test helper: run `SolvePixiEnvironmentKey` via the compute engine
/// and map errors back to the shape legacy tests expect.
async fn run_pixi_solve(
    dispatcher: &CommandDispatcher,
    spec: SolvePixiEnvironmentSpec,
) -> Result<Vec<pixi_record::PixiRecord>, CommandDispatcherError<SolvePixiEnvironmentError>> {
    use pixi_command_dispatcher::ComputeResultExt;
    use pixi_command_dispatcher::keys::SolvePixiEnvironmentKey;
    let records_arc = dispatcher
        .engine()
        .compute(&SolvePixiEnvironmentKey::new(spec))
        .await
        .map_err_into_dispatcher(std::convert::identity)?;
    Ok((*records_arc).clone())
}

/// Convert a solved `PixiRecord` into the `UnresolvedPixiRecord` shape
/// `SolvePixiEnvironmentSpec::installed` wants, so a prior solve's
/// output can be fed back as an installed-hint for the next solve.
fn to_unresolved(record: pixi_record::PixiRecord) -> pixi_record::UnresolvedPixiRecord {
    record.into()
}

/// Wraps `(channels, build_env)` into an `EnvironmentRef::Ephemeral` so
/// test construction sites stay terse. Uses default (strict) channel
/// priority, empty variants, no `exclude_newer`.
fn env_ref_of(channels: Vec<ChannelUrl>, build_environment: BuildEnvironment) -> EnvironmentRef {
    EnvironmentRef::Ephemeral(EphemeralEnv::new(
        "test",
        EnvironmentSpec {
            channels,
            build_environment,
            variants: VariantConfig::default(),
            exclude_newer: None,
            channel_priority: Default::default(),
        },
    ))
}

/// Returns a default set of cache directories for the test.
fn default_cache_dirs() -> CacheDirs {
    let cache_dir = pixi_config::get_cache_dir().unwrap();
    CacheDirs::new(to_abs_dir(cache_dir))
}

/// Returns the tool platform that is appropriate for the current platform.
///
/// Specifically, it normalizes `WinArm64` to `Win64` to increase compatibility.
/// TODO: Once conda-forge supports `WinArm64`, we can remove this
/// normalization.
fn tool_platform() -> (Platform, Vec<GenericVirtualPackage>) {
    let platform = match Platform::current() {
        Platform::WinArm64 => Platform::Win64,
        platform => platform,
    };
    let virtual_packages = VirtualPackages::detect(&VirtualPackageOverrides::default())
        .unwrap()
        .into_generic_virtual_packages()
        .collect();
    (platform, virtual_packages)
}

/// Returns the path to the root of the workspace.
fn cargo_workspace_dir() -> &'static Path {
    Path::new(env!("CARGO_WORKSPACE_DIR"))
}

/// Returns the path to the `tests/data/workspaces` directory in the repository.
fn workspaces_dir() -> PathBuf {
    cargo_workspace_dir().join("tests/data/workspaces")
}

/// Recursively copies a directory from `src` to `dst`.
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Returns the default build environment to use for tests.
fn default_build_environment() -> BuildEnvironment {
    let (tool_platform, tool_virtual_packages) = tool_platform();
    BuildEnvironment::simple(tool_platform, tool_virtual_packages)
}

fn dummy_sha() -> Sha256Hash {
    Sha256::digest(b"pixi-url-cache-test")
}

fn prepare_cached_checkout(cache_root: &Path, sha: Sha256Hash) -> PathBuf {
    let checkout_dir = cache_root.join("checkouts").join(format!("{sha:x}"));
    fs::create_dir_all(&checkout_dir).unwrap();
    fs::write(checkout_dir.join("payload.txt"), "cached contents").unwrap();
    fs::write(checkout_dir.join(".pixi-url-ready"), "ready").unwrap();
    checkout_dir
}

fn hello_world_archive() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/url/hello_world.zip")
}

fn file_url_for_test(tempdir: &TempDir, name: &str) -> Url {
    let path = tempdir.path().join(name);
    fs::copy(hello_world_archive(), &path).unwrap();
    Url::from_file_path(&path).unwrap()
}

/// Build a minimal [`ComputeEngine`] sufficient for URL-checkout tests.
///
/// Populates the data store with only the entries the `CheckoutUrl` Key
/// reads (url resolver, download client, cache dirs, url-checkout
/// semaphore, optional reporter), and installs the reporter-context
/// spawn hook so lifecycle events carry context across task spawns.
fn url_test_engine(
    cache_dirs: CacheDirs,
    reporter: Option<Arc<dyn pixi_command_dispatcher::Reporter>>,
    sequential: bool,
    max_concurrent: Option<usize>,
) -> pixi_compute_engine::ComputeEngine {
    let mut builder = ComputeEngine::builder()
        .sequential_branches(sequential)
        .with_data(pixi_url::UrlResolver::default())
        .with_data(rattler_networking::LazyClient::default())
        .with_data(cache_dirs)
        .with_spawn_hook(Arc::new(ReporterContextSpawnHook));
    if let Some(reporter) = reporter {
        builder = builder.with_data(reporter);
    }
    if let Some(n) = max_concurrent {
        builder = builder.with_data(UrlCheckoutSemaphore(Arc::new(Semaphore::new(n))));
    }
    builder.build()
}

#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
pub async fn simple_test() {
    use pixi_test_utils::GitRepoFixture;

    // Create a local git repo from our fixture
    let git_repo = GitRepoFixture::new("multi-output-recipe");

    // Use a local channel (backend_channel_1 has pixi-build-api-version which is needed for builds)
    let channel_dir = cargo_workspace_dir().join("tests/data/channels/channels/backend_channel_1");
    let channel_url: ChannelUrl = Url::from_directory_path(&channel_dir).unwrap().into();

    let (reporter, events) = EventReporter::new();
    let (tool_platform, tool_virtual_packages) = tool_platform();
    let tempdir = tempfile::tempdir().unwrap();
    let prefix_dir = tempdir.path().join("prefix");
    let dispatcher = CommandDispatcher::builder()
        .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
        .with_reporter(reporter)
        .with_executor(Executor::Serial)
        .with_tool_platform(tool_platform, tool_virtual_packages.clone())
        .finish();

    let build_env = default_build_environment();

    let records = run_pixi_solve(
        &dispatcher,
        SolvePixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([(
                "foobar-desktop".parse().unwrap(),
                GitSpec {
                    git: git_repo.url.parse().unwrap(),
                    rev: Some(GitReference::Rev(git_repo.commits[0].clone())),
                    subdirectory: Subdirectory::try_from("recipe").unwrap(),
                }
                .into(),
            )]),
            env_ref: env_ref_of(vec![channel_url.clone()], build_env.clone()),
            ..empty_pixi_env_spec()
        },
    )
    .await
    .unwrap();

    dispatcher
        .install_pixi_environment(InstallPixiEnvironmentSpec {
            name: "test-env".to_owned(),
            records: records.iter().cloned().map(Into::into).collect(),
            prefix: Prefix::create(&prefix_dir).unwrap(),
            installed: None,
            build_environment: build_env,
            ignore_packages: None,
            force_reinstall: Default::default(),
            exclude_newer: None,
            channels: vec![channel_url],
            variant_configuration: None,
            variant_files: None,
        })
        .await
        .unwrap();

    println!(
        "Built the environment successfully: {}",
        prefix_dir.display()
    );

    let event_tree = EventTree::from(events);

    // Redact temp paths and git hashes for stable snapshots.
    //
    // The first regex uses `[^@\n]+` rather than `[^@]+` to prevent the
    // greedy match from spanning multiple tree lines: the event tree
    // emits several URLs that all end in `/multi-output-recipe/`, and a
    // newline-crossing greedy `[^@]+` would collapse the span from one
    // URL's `file:///` through the next line's `@<commit>`, eating the
    // `?subdirectory=&rev=` query of the outer URL.
    let output = event_tree.to_string();
    let output = regex::Regex::new(r"file:///[^@\n]+/multi-output-recipe/")
        .unwrap()
        .replace_all(&output, "file://[LOCAL_GIT_REPO]");
    let output = regex::Regex::new(r"rev=[a-z0-9]+")
        .unwrap()
        .replace_all(&output, "rev=[GIT_REF]");
    let output = regex::Regex::new(r"[#@][a-f0-9]{40}")
        .unwrap()
        .replace_all(&output, "#[GIT_HASH]");
    // Redact the host platform that the solve label embeds, so the
    // snapshot is stable across hosts.
    let output = output.replace(&tool_platform.to_string(), "[PLATFORM]");
    insta::assert_snapshot!(output);
}

#[tokio::test]
pub async fn instantiate_backend_with_compatible_api_version() {
    let backend_name = PackageName::new_unchecked("backend-with-compatible-api-version");
    let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap();
    let channel_dir = root_dir.join("tests/data/channels/channels/backend_channel_1");

    let dispatcher = CommandDispatcher::builder()
        .with_cache_dirs(default_cache_dirs())
        .with_executor(Executor::Serial)
        .finish();

    dispatcher
        .instantiate_tool_environment(InstantiateToolEnvironmentSpec::new(
            backend_name,
            PixiSpec::Version(VersionSpec::Any),
            Vec::from([Url::from_directory_path(channel_dir).unwrap().into()]),
        ))
        .await
        .unwrap();
}

#[tokio::test]
pub async fn instantiate_backend_without_compatible_api_version() {
    let backend_name = PackageName::new_unchecked("backend-without-compatible-api-version");
    let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap();
    let channel_dir = root_dir.join("tests/data/channels/channels/backend_channel_1");

    let dispatcher = CommandDispatcher::builder()
        .with_cache_dirs(default_cache_dirs())
        .with_executor(Executor::Serial)
        .finish();

    let err = dispatcher
        .instantiate_tool_environment(InstantiateToolEnvironmentSpec::new(
            backend_name,
            PixiSpec::Version(VersionSpec::Any),
            Vec::from([Url::from_directory_path(channel_dir).unwrap().into()]),
        ))
        .await
        .unwrap_err();

    insta::assert_snapshot!(format_diagnostic(&err));
}

#[tokio::test]
pub async fn instantiate_backend_with_compatible_api_version_respects_exclude_newer() {
    let backend_name = PackageName::new_unchecked("backend-with-compatible-api-version");
    let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap();
    let channel_dir = root_dir.join("tests/data/channels/channels/backend_channel_1");

    let dispatcher = CommandDispatcher::builder()
        .with_cache_dirs(default_cache_dirs())
        .with_executor(Executor::Serial)
        .finish();

    dispatcher
        .instantiate_tool_environment(InstantiateToolEnvironmentSpec::new(
            backend_name.clone(),
            PixiSpec::Version(VersionSpec::Any),
            Vec::from([Url::from_directory_path(channel_dir.clone())
                .unwrap()
                .into()]),
        ))
        .await
        .expect("backend should instantiate without exclude-newer");

    let err = dispatcher
        .instantiate_tool_environment(InstantiateToolEnvironmentSpec {
            exclude_newer: Some(ResolvedExcludeNewer::from_datetime(
                "2025-01-01T00:00:00Z".parse().unwrap(),
            )),
            ..InstantiateToolEnvironmentSpec::new(
                backend_name,
                PixiSpec::Version(VersionSpec::Any),
                Vec::from([Url::from_directory_path(channel_dir).unwrap().into()]),
            )
        })
        .await
        .unwrap_err();

    let rendered = format_diagnostic(&err);
    assert!(rendered.contains("backend-with-compatible-api-version"));
}

#[tokio::test]
pub async fn instantiate_backend_with_compatible_api_version_honors_exclude_newer_overrides() {
    let backend_name = PackageName::new_unchecked("backend-with-compatible-api-version");
    let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap();
    let channel_dir = root_dir.join("tests/data/channels/channels/backend_channel_1");
    let channel_url = Url::from_directory_path(channel_dir).unwrap().into();
    let allowed_cutoff = "2026-12-31T00:00:00Z".parse().unwrap();

    let dispatcher = CommandDispatcher::builder()
        .with_cache_dirs(default_cache_dirs())
        .with_executor(Executor::Serial)
        .finish();

    dispatcher
        .instantiate_tool_environment(InstantiateToolEnvironmentSpec {
            exclude_newer: Some(
                ResolvedExcludeNewer::from_datetime("2025-01-01T00:00:00Z".parse().unwrap())
                    .with_package_cutoff(backend_name.clone(), allowed_cutoff)
                    .with_package_cutoff(
                        PackageName::new_unchecked("pixi-build-api-version"),
                        allowed_cutoff,
                    ),
            ),
            ..InstantiateToolEnvironmentSpec::new(
                backend_name,
                PixiSpec::Version(VersionSpec::Any),
                Vec::from([channel_url]),
            )
        })
        .await
        .expect("backend should instantiate when backend packages are explicitly overridden");
}

/// When two identical tool env instantiations are queued concurrently and the
/// operation fails, the dispatcher sends the failure to one waiter and cancels
/// the others. This verifies cancellation without network access.
#[tokio::test]
pub async fn instantiate_backend_without_compatible_api_version_cancels_duplicates() {
    use pixi_command_dispatcher::CommandDispatcherError;

    let backend_name = PackageName::new_unchecked("backend-without-compatible-api-version");
    let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap();
    let channel_dir = root_dir.join("tests/data/channels/channels/backend_channel_1");

    let dispatcher = CommandDispatcher::builder()
        .with_cache_dirs(default_cache_dirs())
        .with_executor(Executor::Serial)
        .finish();

    // Build the spec once and issue two identical concurrent requests.
    let spec = InstantiateToolEnvironmentSpec::new(
        backend_name,
        PixiSpec::Version(VersionSpec::Any),
        Vec::from([Url::from_directory_path(channel_dir).unwrap().into()]),
    );

    let (r1, r2) = tokio::join!(
        dispatcher.instantiate_tool_environment(spec.clone()),
        dispatcher.instantiate_tool_environment(spec),
    );

    // Both results should be failures since errors are now cloned and sent to all channels.
    let is_cancelled = |r: &Result<
        _,
        CommandDispatcherError<pixi_command_dispatcher::InstantiateToolEnvironmentError>,
    >| matches!(r, Err(CommandDispatcherError::Cancelled));
    let is_failed = |r: &Result<
        _,
        CommandDispatcherError<pixi_command_dispatcher::InstantiateToolEnvironmentError>,
    >| matches!(r, Err(CommandDispatcherError::Failed(_)));

    let cancelled_count = usize::from(is_cancelled(&r1)) + usize::from(is_cancelled(&r2));
    let failed_count = usize::from(is_failed(&r1)) + usize::from(is_failed(&r2));

    assert_eq!(
        cancelled_count, 0,
        "no requests should be cancelled - errors are cloned to all channels"
    );
    assert_eq!(
        failed_count, 2,
        "both requests should fail with the same error"
    );
}

/// Dropping the returned future should cancel the background task promptly.
/// This test verifies that behavior by:
/// - installing a compatible backend from a local channel (no network)
/// - starting instantiate_tool_environment with a reporter
/// - waiting until the background task is started, then immediately dropping
///   the caller future (via abort)
/// - ensuring no installation starts and we still see a clean finish event
#[tokio::test]
pub async fn dropping_future_cancels_background_task() {
    // Arrange a dispatcher with an event reporter for synchronization.
    let (reporter, events) = EventReporter::new();
    let root_dir = cargo_workspace_dir();
    let channel_dir = root_dir.join("tests/data/channels/channels/backend_channel_1");

    // Use an isolated cache dir. The ephemeral-env compute key takes a
    // fast path when it finds a `.pixi-ephemeral-cache.json` marker in
    // the target prefix, returning the cached records before any
    // `CondaSolveStarted` event fires. With a shared cache that marker
    // can already be present from a previous run of this test (or a
    // stray `pixi run` reusing the same backend), so the test waits for
    // a solve that never happens. A fresh tempdir guarantees a clean
    // prefix and forces the slow path the test actually wants to
    // observe.
    let cache_tempdir = TempDir::new().unwrap();
    let cache_dirs = CacheDirs::new(to_abs_dir(cache_tempdir.path().to_path_buf()));

    let dispatcher = CommandDispatcher::builder()
        .with_cache_dirs(cache_dirs)
        .with_reporter(reporter)
        .with_executor(Executor::Serial)
        .finish();

    // Use a backend that has a compatible API version so solve would succeed
    // and the task would progress to installation if not cancelled.
    let spec = InstantiateToolEnvironmentSpec::new(
        PackageName::new_unchecked("backend-with-compatible-api-version"),
        PixiSpec::Version(VersionSpec::Any),
        Vec::from([Url::from_directory_path(channel_dir).unwrap().into()]),
    );

    // Hold the write lock for the target tool prefix so installation cannot
    // progress even if solve completes; this makes the test deterministic.
    let prefix_dir = dispatcher
        .cache_dirs()
        .build_backends()
        .join(spec.cache_key());
    let mut write_guard = pixi_utils::AsyncPrefixGuard::new(prefix_dir.as_std_path())
        .await
        .unwrap()
        .write()
        .await
        .unwrap();

    // Spawn the instantiate future and wait until the background marks it started.
    let dispatcher = dispatcher.clone();
    let handle = tokio::spawn(async move { dispatcher.instantiate_tool_environment(spec).await });

    // `instantiate_tool_environment` goes through `EphemeralEnvKey`, which
    // runs a binary-only conda solve before touching the prefix. Waiting
    // for `CondaSolveStarted` proves the background task entered its
    // compute body. The timeout is generous on purpose: cold caches on
    // slow CI runners can take many seconds to spin up the dispatcher
    // and reach the solve, and the test doesn't care about latency, only
    // ordering (start before cancel).
    let started = events
        .wait_until_matches(
            |e| matches!(e, Event::CondaSolveStarted { .. }),
            std::time::Duration::from_secs(30),
        )
        .await
        .is_ok();
    assert!(started, "instantiate task did not start in time");

    // Act: immediately drop the caller future by aborting the task.
    handle.abort();

    // Release our write lock (after cancellation). If cancellation worked,
    // the background task should not proceed into installation.
    write_guard.begin().await.unwrap();
    drop(write_guard);

    // The aborted task should unwind promptly once the prefix lock is
    // released and drop propagates through the compute engine. Same
    // timeout reasoning as the start wait above.
    let join = tokio::time::timeout(std::time::Duration::from_secs(30), handle).await;
    assert!(
        join.is_ok(),
        "instantiate task did not finish promptly after cancellation"
    );
}

#[tokio::test]
pub async fn test_cycle() {
    // Setup a reporter that allows us to trace the steps taken by the command
    // dispatcher.
    let (reporter, events) = EventReporter::new();

    // Use a fixed solve platform so the snapshot is stable across
    // host platforms. The tool platform still reflects the current
    // host (the dispatcher needs it for its own tool env), but the
    // solve uses a deterministic BuildEnvironment.
    let (tool_platform, tool_virtual_packages) = tool_platform();
    let root_dir = workspaces_dir().join("cycle");
    let tempdir = tempfile::tempdir().unwrap();
    let dispatcher = CommandDispatcher::builder()
        .with_root_dir(to_abs_dir(root_dir.clone()))
        .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
        .with_reporter(reporter)
        .with_executor(Executor::Serial)
        .with_tool_platform(tool_platform, tool_virtual_packages)
        .with_backend_overrides(BackendOverride::from_memory(
            PassthroughBackend::instantiator(),
        ))
        .finish();

    // Solve an environment with package_a. This should introduce a cycle because
    // package_a depends on package_b, which depends on package_a.
    let error = run_pixi_solve(
        &dispatcher,
        SolvePixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([(
                "package_a".parse().unwrap(),
                PathSpec {
                    path: "package_a".into(),
                }
                .into(),
            )]),
            env_ref: env_ref_of(vec![], BuildEnvironment::simple(Platform::Linux64, vec![])),
            ..empty_pixi_env_spec()
        },
    )
    .await
    .expect_err("expected a cycle error");

    // Output the error and the event tree to a snapshot for debugging.
    let event_tree = EventTree::from(events);
    insta::assert_snapshot!(format!(
        "ERROR:\n{}\n\nTRACE:\n{}",
        format_diagnostic(&error),
        event_tree.to_string()
    ));
}

/// Three-package cycle spanning two different dependency kinds:
/// `package_a`'s host-deps → `package_b`'s run-deps → `package_c`'s
/// host-deps → back to `package_a`. Exercises the cycle renderer on
/// a three-frame ring mixing host and run edges.
#[tokio::test]
pub async fn test_cycle_three_packages() {
    let (reporter, events) = EventReporter::new();

    // Use a fixed solve platform so the snapshot is stable across hosts.
    let (tool_platform, tool_virtual_packages) = tool_platform();
    let root_dir = workspaces_dir().join("cycle_three");
    let tempdir = tempfile::tempdir().unwrap();
    let dispatcher = CommandDispatcher::builder()
        .with_root_dir(to_abs_dir(root_dir.clone()))
        .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
        .with_reporter(reporter)
        .with_executor(Executor::Serial)
        .with_tool_platform(tool_platform, tool_virtual_packages)
        .with_backend_overrides(BackendOverride::from_memory(
            PassthroughBackend::instantiator(),
        ))
        .finish();

    // Solve an environment with package_a. The cycle is
    // A (host) -> B (run) -> C (host) -> A.
    let error = run_pixi_solve(
        &dispatcher,
        SolvePixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([(
                "package_a".parse().unwrap(),
                PathSpec {
                    path: "package_a".into(),
                }
                .into(),
            )]),
            env_ref: env_ref_of(vec![], BuildEnvironment::simple(Platform::Linux64, vec![])),
            ..empty_pixi_env_spec()
        },
    )
    .await
    .expect_err("expected a cycle error");

    let event_tree = EventTree::from(events);
    insta::assert_snapshot!(format!(
        "ERROR:\n{}\n\nTRACE:\n{}",
        format_diagnostic(&error),
        event_tree.to_string()
    ));
}

/// Tests that a stale host dependency triggers a rebuild of both the stale
/// package and any package that specifies it as a host dependency.
#[tokio::test]
pub async fn test_stale_host_dependency_triggers_rebuild() {
    // Copy workspace to temp directory so we can modify files without affecting other tests
    let source_dir = workspaces_dir().join("host-dependency");
    let tempdir = tempfile::tempdir().unwrap();
    let root_dir = tempdir.path().join("workspace");
    copy_dir_recursive(&source_dir, &root_dir).unwrap();
    let (tool_platform, tool_virtual_packages) = tool_platform();
    let build_env = BuildEnvironment::simple(tool_platform, tool_virtual_packages.clone());
    let build_command_dispatcher = || {
        CommandDispatcher::builder()
            .with_root_dir(to_abs_dir(root_dir.clone()))
            .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
            .with_executor(Executor::Serial)
            .with_tool_platform(tool_platform, tool_virtual_packages.clone())
            .with_backend_overrides(BackendOverride::from_memory(
                PassthroughBackend::instantiator(),
            ))
    };

    let (reporter, first_events) = EventReporter::new();
    let dispatcher = build_command_dispatcher().with_reporter(reporter).finish();

    // Solve an environment with package-a which will have a host dependency on
    // package-b, and package-c which has a run dependency on package-b.
    let records = run_pixi_solve(
        &dispatcher,
        SolvePixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([
                (
                    "package-a".parse().unwrap(),
                    PathSpec::new("package-a").into(),
                ),
                (
                    "package-c".parse().unwrap(),
                    PathSpec::new("package-c").into(),
                ),
            ]),
            env_ref: env_ref_of(vec![], build_env.clone()),
            ..empty_pixi_env_spec()
        },
    )
    .await
    .map_err(|e| format_diagnostic(&e))
    .expect("expected solve to succeed");

    // package-b should not be part of the solution, its only used as a host
    // dependency.
    let package_names = records
        .iter()
        .map(|r| r.name().as_normalized())
        .sorted()
        .collect::<Vec<_>>();
    assert_eq!(
        package_names,
        vec!["package-a", "package-b", "package-c"],
        "Expected package-a, package-b and package-c to be part of the solution"
    );

    // Install the environment to a temporary prefix.
    let prefix = Prefix::create(tempdir.path().join("prefix")).unwrap();
    let _ = dispatcher
        .install_pixi_environment(InstallPixiEnvironmentSpec {
            build_environment: build_env.clone(),
            ..InstallPixiEnvironmentSpec::new(records.clone(), prefix.clone())
        })
        .await
        .map_err(|e| format_diagnostic(&e))
        .unwrap();

    // Explicitly drop the dispatcher to ensure all caches are flushed.
    let first_events = first_events.take();
    drop(dispatcher);

    // TOUCH a file that triggers a rebuild of package-b. package-b defines a build
    // glob that will include this file. Any new file that matches the glob should
    // trigger a rebuild.
    let _touch_temp_file = tempfile::Builder::new()
        .prefix("TOUCH")
        .tempfile_in(root_dir.join("package-b"))
        .unwrap();

    // Construct a new command dispatcher (as if the program is restarted).
    let (reporter, second_events) = EventReporter::new();
    let dispatcher = build_command_dispatcher().with_reporter(reporter).finish();

    // Rerun the installation of the environment.
    let _ = dispatcher
        .install_pixi_environment(InstallPixiEnvironmentSpec {
            build_environment: build_env.clone(),
            ..InstallPixiEnvironmentSpec::new(records.clone(), prefix)
        })
        .await
        .map_err(|e| format_diagnostic(&e))
        .unwrap();

    // Get all the events that happened.
    let second_events = second_events.take();
    let event_tree = EventTree::new(first_events.iter().chain(second_events.iter())).to_string();
    eprintln!("{event_tree}");

    // Ensure that both package-a and package-b were rebuilt.
    let rebuild_packages = second_events
        .iter()
        .filter_map(|event| match event {
            event_reporter::Event::BackendSourceBuildQueued { package, .. } => {
                Some(package.as_str())
            }
            _ => None,
        })
        .sorted()
        .collect::<Vec<_>>();

    assert_eq!(
        rebuild_packages,
        vec!["package-b"],
        "Expected only package-b to be rebuilt"
    );
}

#[tokio::test]
#[ignore = "EphemeralEnvKey rejects source specs; re-enable once source deps in \
            ephemeral envs are supported"]
pub async fn instantiate_backend_with_from_source() {
    // Use existing backend_channel_1 which has pixi-build-api-version package with actual .conda files
    let channel_dir = cargo_workspace_dir().join("tests/data/channels/channels/backend_channel_1");
    let channel_url = url::Url::from_directory_path(&channel_dir).unwrap();

    // Copy source-backends workspace to temp directory so we can modify the channel
    let source_dir = workspaces_dir().join("source-backends");
    let tmp_dir = tempfile::tempdir().unwrap();
    let root_dir = tmp_dir.path().to_path_buf();
    copy_dir_recursive(&source_dir, &root_dir).unwrap();

    // Update workspace pixi.toml to use local channel instead of conda-forge
    let workspace_toml = root_dir.join("pixi.toml");
    let content = fs_err::read_to_string(&workspace_toml).unwrap();
    let content = content.replace("conda-forge", channel_url.as_str());
    fs_err::write(&workspace_toml, content).unwrap();

    let dispatcher = CommandDispatcher::builder()
        .with_root_dir(to_abs_dir(root_dir.clone()))
        .with_cache_dirs(CacheDirs::new(to_abs_dir(root_dir.join(".pixi"))))
        .with_executor(Executor::Serial)
        .with_backend_overrides(BackendOverride::InMemory(
            InMemoryOverriddenBackends::Specified(HashMap::from_iter([(
                "in-memory".to_string(),
                PassthroughBackend::instantiator().into(),
            )])),
        ))
        .finish();

    // Use PixiSpec::Path to test path-based resolution and installation
    let err = dispatcher
        .instantiate_tool_environment(InstantiateToolEnvironmentSpec::new(
            PackageName::new_unchecked("package-d"),
            PathSpec::new("package-d").into(),
            Vec::default(),
        ))
        .await
        .err()
        .unwrap();

    insta::assert_debug_snapshot!(err);
}

/// Tests that `dev_source_metadata` correctly retrieves all outputs from a dev source
/// and creates DevSourceRecords with combined dependencies.
#[tokio::test]
pub async fn test_dev_source_metadata() {
    use pixi_command_dispatcher::{BuildBackendMetadataSpec, DevSourceMetadataSpec};
    use pixi_record::PinnedPathSpec;

    // Setup: Create a dispatcher with the in-memory backend
    let root_dir = workspaces_dir().join("dev-sources");
    let tempdir = tempfile::tempdir().unwrap();
    let (tool_platform, tool_virtual_packages) = tool_platform();

    let dispatcher = CommandDispatcher::builder()
        .with_root_dir(to_abs_dir(root_dir.clone()))
        .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
        .with_executor(Executor::Serial)
        .with_tool_platform(tool_platform, tool_virtual_packages.clone())
        .with_backend_overrides(BackendOverride::from_memory(
            PassthroughBackend::instantiator(),
        ))
        .finish();

    // Pin the source spec to a path
    let pinned_source = PinnedPathSpec {
        path: "test-package".into(),
    }
    .into();

    // Create the spec for dev source metadata
    let spec = DevSourceMetadataSpec {
        package_name: PackageName::new_unchecked("test-package"),
        backend_metadata: BuildBackendMetadataSpec {
            manifest_source: pinned_source,
            preferred_build_source: None,
            env_ref: env_ref_of(
                vec![],
                BuildEnvironment::simple(tool_platform, tool_virtual_packages),
            ),
        },
    };

    // Act: Get the dev source metadata
    let result = dispatcher
        .dev_source_metadata(spec)
        .await
        .map_err(|e| format_diagnostic(&e))
        .expect("dev_source_metadata should succeed");

    // Assert: Should have one record for test-package
    assert_eq!(
        result.records.len(),
        1,
        "Should have one record for test-package"
    );

    let record = &result.records[0];

    // Verify the record has the correct name
    assert_eq!(
        record.name.as_source(),
        "test-package",
        "Record should be for test-package"
    );

    // Verify all dependencies are combined (build + host + run)
    // From the test data: build (cmake, make), host (zlib, openssl), run (python, numpy)
    let dep_names: Vec<_> = record
        .dependencies
        .names()
        .map(|name| name.as_normalized())
        .sorted()
        .collect();

    assert_eq!(
        dep_names,
        vec!["cmake", "make", "numpy", "openssl", "python", "zlib"],
        "All dependencies (build, host, run) should be combined"
    );

    // Verify constraints are empty (test package has no constraints)
    assert!(
        record.constraints.is_empty(),
        "Test package has no constraints"
    );
}

/// Tests that `dev_source_metadata` returns an error when requesting a package
/// that is not provided by the source.
#[tokio::test]
pub async fn test_dev_source_metadata_package_not_provided() {
    use pixi_command_dispatcher::{
        BuildBackendMetadataSpec, CommandDispatcherError, DevSourceMetadataError,
        DevSourceMetadataSpec, PackageNotProvidedError,
    };
    use pixi_record::PinnedPathSpec;

    // Setup: Create a dispatcher with the in-memory backend
    let root_dir = workspaces_dir().join("dev-sources");
    let tempdir = tempfile::tempdir().unwrap();
    let (tool_platform, tool_virtual_packages) = tool_platform();

    let dispatcher = CommandDispatcher::builder()
        .with_root_dir(to_abs_dir(root_dir.clone()))
        .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
        .with_executor(Executor::Serial)
        .with_tool_platform(tool_platform, tool_virtual_packages.clone())
        .with_backend_overrides(BackendOverride::from_memory(
            PassthroughBackend::instantiator(),
        ))
        .finish();

    // Pin the source spec to test-package which provides "test-package"
    let pinned_source = PinnedPathSpec {
        path: "test-package".into(),
    }
    .into();

    // Request a package name that doesn't exist in the source
    let spec = DevSourceMetadataSpec {
        package_name: PackageName::new_unchecked("non-existent-package"),
        backend_metadata: BuildBackendMetadataSpec {
            manifest_source: pinned_source,
            preferred_build_source: None,
            env_ref: env_ref_of(
                vec![],
                BuildEnvironment::simple(tool_platform, tool_virtual_packages),
            ),
        },
    };

    // Act: Get the dev source metadata - should fail
    let result = dispatcher.dev_source_metadata(spec).await;

    // Assert: Should return PackageNotProvided error
    let err = result.expect_err("should fail when package is not provided by source");

    match err {
        CommandDispatcherError::Failed(DevSourceMetadataError::PackageNotProvided(
            PackageNotProvidedError { name, .. },
        )) => {
            assert_eq!(
                name.as_source(),
                "non-existent-package",
                "Error should contain the requested package name"
            );
        }
        other => panic!("expected PackageNotProvided error, got: {other}"),
    }
}

/// Tests that the PassthroughBackend generates multiple outputs based on variant configurations
/// when dependencies have "*" version requirements.
#[tokio::test]
pub async fn test_dev_source_metadata_with_variants() {
    use pixi_command_dispatcher::{BuildBackendMetadataSpec, DevSourceMetadataSpec};
    use pixi_record::PinnedPathSpec;
    use std::collections::BTreeMap;

    // Setup: Create a dispatcher with the in-memory backend
    let root_dir = workspaces_dir().join("dev-sources");
    let tempdir = tempfile::tempdir().unwrap();
    let (tool_platform, tool_virtual_packages) = tool_platform();

    let dispatcher = CommandDispatcher::builder()
        .with_root_dir(to_abs_dir(root_dir.clone()))
        .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
        .with_executor(Executor::Serial)
        .with_tool_platform(tool_platform, tool_virtual_packages.clone())
        .with_backend_overrides(BackendOverride::from_memory(
            PassthroughBackend::instantiator(),
        ))
        .finish();

    // Pin the source spec to a path
    let pinned_source = PinnedPathSpec {
        path: "variant-package".into(),
    }
    .into();

    // Create variant configuration for python and numpy
    let mut variant_config = BTreeMap::new();
    variant_config.insert(
        "python".to_string(),
        vec!["3.10".to_string().into(), "3.11".to_string().into()],
    );
    variant_config.insert(
        "numpy".to_string(),
        vec!["1.0".to_string().into(), "2.0".to_string().into()],
    );

    // Create the spec for dev source metadata with variants
    let spec = DevSourceMetadataSpec {
        package_name: PackageName::new_unchecked("variant-package"),
        backend_metadata: BuildBackendMetadataSpec {
            manifest_source: pinned_source,
            preferred_build_source: None,
            env_ref: EnvironmentRef::Ephemeral(EphemeralEnv::new(
                "variant-test",
                EnvironmentSpec {
                    channels: vec![],
                    build_environment: BuildEnvironment::simple(
                        tool_platform,
                        tool_virtual_packages,
                    ),
                    variants: VariantConfig {
                        variant_configuration: variant_config,
                        variant_files: Vec::new(),
                    },
                    exclude_newer: None,
                    channel_priority: Default::default(),
                },
            )),
        },
    };

    // Act: Get the dev source metadata
    let result = dispatcher
        .dev_source_metadata(spec)
        .await
        .map_err(|e| format_diagnostic(&e))
        .expect("dev_source_metadata should succeed");

    // Assert: Should have 4 records (2 python versions × 2 numpy versions)
    assert_eq!(
        result.records.len(),
        4,
        "Should have 4 records for all variant combinations"
    );

    // Collect all variant combinations
    let variants: Vec<_> = result
        .records
        .iter()
        .map(|record| {
            let python = record
                .variants
                .get("python")
                .map(|s| s.to_string())
                .unwrap_or("none".to_string());
            let numpy = record
                .variants
                .get("numpy")
                .map(|s| s.to_string())
                .unwrap_or("none".to_string());
            (python, numpy)
        })
        .sorted()
        .collect();

    // Verify all expected combinations are present
    assert_eq!(
        variants,
        vec![
            ("3.10".to_string(), "1.0".to_string()),
            ("3.10".to_string(), "2.0".to_string()),
            ("3.11".to_string(), "1.0".to_string()),
            ("3.11".to_string(), "2.0".to_string()),
        ],
        "All variant combinations should be generated"
    );

    // Verify each record has the correct variant metadata
    for record in &result.records {
        assert_eq!(
            record.name.as_source(),
            "variant-package",
            "All records should have the same package name"
        );

        // Verify the variant is properly set in the record
        assert!(
            record.variants.contains_key("python"),
            "Variant should contain python key"
        );
        assert!(
            record.variants.contains_key("numpy"),
            "Variant should contain numpy key"
        );

        // Verify python and numpy are in dependencies (all combined)
        let dep_names: Vec<_> = record
            .dependencies
            .names()
            .map(|n| n.as_normalized())
            .sorted()
            .collect();

        assert!(
            dep_names.contains(&"python"),
            "Python should be in dependencies"
        );
        assert!(
            dep_names.contains(&"numpy"),
            "Numpy should be in dependencies"
        );
    }
}

/// Tests that forcing a rebuild of a package will ignore UpToDate cache status from previous builds.
#[tokio::test]
pub async fn test_force_rebuild() {
    let root_dir = workspaces_dir().join("host-dependency");
    let tempdir = tempfile::tempdir().unwrap();
    let (tool_platform, tool_virtual_packages) = tool_platform();
    let build_env = BuildEnvironment::simple(tool_platform, tool_virtual_packages.clone());
    let build_command_dispatcher = || {
        CommandDispatcher::builder()
            .with_root_dir(to_abs_dir(root_dir.clone()))
            .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
            .with_executor(Executor::Serial)
            .with_tool_platform(tool_platform, tool_virtual_packages.clone())
            .with_backend_overrides(BackendOverride::from_memory(
                PassthroughBackend::instantiator(),
            ))
    };

    let (reporter, events) = EventReporter::new();
    let dispatcher = build_command_dispatcher().with_reporter(reporter).finish();

    // Made a source build of package-b CacheStatus::UpToDate by installing the environment once.
    let records = run_pixi_solve(
        &dispatcher,
        SolvePixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([
                (
                    "package-a".parse().unwrap(),
                    PathSpec::new("package-a").into(),
                ),
                (
                    "package-c".parse().unwrap(),
                    PathSpec::new("package-c").into(),
                ),
            ]),
            env_ref: env_ref_of(vec![], build_env.clone()),
            ..empty_pixi_env_spec()
        },
    )
    .await
    .map_err(|e| format_diagnostic(&e))
    .expect("expected solve to succeed");

    // Install the environment to a temporary prefix.
    // we know will have CacheStatus::New package-b after this
    let prefix = Prefix::create(tempdir.path().join("prefix")).unwrap();
    dispatcher
        .install_pixi_environment(InstallPixiEnvironmentSpec {
            build_environment: build_env.clone(),
            ..InstallPixiEnvironmentSpec::new(records.clone(), prefix.clone())
        })
        .await
        .map_err(|e| format_diagnostic(&e))
        .unwrap();

    let first_events = events.take();

    dispatcher.clear_reporter().await;

    // Now we want to rebuild package-b by forcing a rebuild. The
    // artifact-cache invalidation is the caller's responsibility:
    // install_pixi_environment's `force_reinstall` only drives the
    // prefix installer's reinstall semantics for already-resolved
    // records. Clear the source-build cache first to force
    // SourceBuildKey to see a miss.
    let mut spec = InstallPixiEnvironmentSpec::new(records.clone(), prefix);
    spec.force_reinstall = HashSet::from_iter([PackageName::new_unchecked("package-b")]);
    for name in &spec.force_reinstall {
        dispatcher.clear_source_build_cache(name).unwrap();
    }

    let _ = dispatcher
        .install_pixi_environment(InstallPixiEnvironmentSpec {
            build_environment: build_env.clone(),
            ..spec.clone()
        })
        .await
        .map_err(|e| format_diagnostic(&e))
        .unwrap();

    // Get all the events that happened.
    let second_events = events.take();
    let event_tree = EventTree::new(first_events.iter().chain(second_events.iter())).to_string();
    eprintln!("{event_tree}");

    // Ensure that package-b was not queued for rebuild since it is a fresh build already.
    let rebuild_packages = second_events
        .iter()
        .filter_map(|event| match event {
            event_reporter::Event::BackendSourceBuildQueued { package, .. } => {
                Some(package.as_str())
            }
            _ => None,
        })
        .sorted()
        .collect::<Vec<_>>();

    assert!(rebuild_packages.is_empty());

    // now drop the dispatcher
    drop(dispatcher);

    // Construct a new command dispatcher (as if the program is restarted).
    let (reporter, second_events) = EventReporter::new();
    let dispatcher = build_command_dispatcher().with_reporter(reporter).finish();

    // Same story across process restarts: the caller clears the cache
    // explicitly when forcing a rebuild.
    for name in &spec.force_reinstall {
        dispatcher.clear_source_build_cache(name).unwrap();
    }

    let _ = dispatcher
        .install_pixi_environment(InstallPixiEnvironmentSpec {
            build_environment: build_env.clone(),
            ..spec
        })
        .await
        .map_err(|e| format_diagnostic(&e))
        .unwrap();

    let second_events = second_events.take();

    // Ensure that package-b was force rebuilt.
    let rebuild_packages = second_events
        .iter()
        .filter_map(|event| match event {
            event_reporter::Event::BackendSourceBuildQueued { package, .. } => {
                Some(package.as_str())
            }
            _ => None,
        })
        .sorted()
        .collect::<Vec<_>>();

    eprintln!("Events after restart:\n");
    let event_tree = EventTree::new(second_events.iter()).to_string();
    eprintln!("{event_tree}");

    assert_eq!(
        rebuild_packages,
        vec!["package-b"],
        "Expected only package-b to be queued for rebuild"
    );

    // now queue again without force rebuild and ensure no builds are queued
    dispatcher.clear_reporter().await;
    spec.force_reinstall = HashSet::new();

    let last_events = events.take();

    // package-b should just reuse cache
    let rebuild_packages = last_events
        .iter()
        .filter_map(|event| match event {
            event_reporter::Event::BackendSourceBuildQueued { package, .. } => {
                Some(package.as_str())
            }
            _ => None,
        })
        .sorted()
        .collect::<Vec<_>>();

    assert!(
        rebuild_packages.is_empty(),
        "Expected no packages to be queued for rebuild"
    );
}

/// Verifies that `force_reinstall` on the ctx install path invalidates
/// the source-build caches and rebuilds the package.
///
/// Uses two separate dispatchers: within a single dispatcher the
/// compute engine's in-memory Key dedup would short-circuit to the
/// cached build even after the on-disk cache is wiped. A CLI re-run
/// spawns a fresh dispatcher (empty in-memory cache) and the
/// force_reinstall hook in `ctx.install_pixi_environment` wipes the
/// on-disk source-build caches for named packages, so the backend
/// gets invoked again.
#[tokio::test]
pub async fn test_compute_ctx_install_force_reinstall_rebuilds_source_package() {
    let root_dir = workspaces_dir().join("host-dependency");
    let tempdir = tempfile::tempdir().unwrap();
    let (tool_platform, tool_virtual_packages) = tool_platform();
    let build_env = BuildEnvironment::simple(tool_platform, tool_virtual_packages.clone());

    let solve_spec = SolvePixiEnvironmentSpec {
        dependencies: DependencyMap::from_iter([
            (
                "package-a".parse().unwrap(),
                PathSpec::new("package-a").into(),
            ),
            (
                "package-c".parse().unwrap(),
                PathSpec::new("package-c").into(),
            ),
        ]),
        env_ref: env_ref_of(vec![], build_env.clone()),
        ..empty_pixi_env_spec()
    };
    let prefix = Prefix::create(tempdir.path().join("prefix")).unwrap();

    // First session: populate the on-disk source-build caches. No
    // observer is attached here since we only care about the second
    // session's backend events.
    let dispatcher = CommandDispatcher::builder()
        .with_root_dir(to_abs_dir(root_dir.clone()))
        .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
        .with_executor(Executor::Serial)
        .with_tool_platform(tool_platform, tool_virtual_packages.clone())
        .with_backend_overrides(BackendOverride::from_memory(
            PassthroughBackend::instantiator(),
        ))
        .finish();
    let records = run_pixi_solve(&dispatcher, solve_spec.clone())
        .await
        .unwrap();
    dispatcher
        .engine()
        .with_ctx(async |ctx| {
            ctx.install_pixi_environment(InstallPixiEnvironmentSpec {
                build_environment: build_env.clone(),
                ..InstallPixiEnvironmentSpec::new(records.clone(), prefix.clone())
            })
            .await
        })
        .await
        .unwrap()
        .unwrap();
    drop(dispatcher);

    // Second session, simulating a CLI rerun with --force-reinstall.
    // Attach the observer here so the assertion below sees only this
    // session's backend events.
    let (instantiator, mut observer) =
        ObservableBackend::instantiator(PassthroughBackend::instantiator());
    let dispatcher = CommandDispatcher::builder()
        .with_root_dir(to_abs_dir(root_dir))
        .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
        .with_executor(Executor::Serial)
        .with_tool_platform(tool_platform, tool_virtual_packages)
        .with_backend_overrides(BackendOverride::from_memory(instantiator))
        .finish();
    let records = run_pixi_solve(&dispatcher, solve_spec).await.unwrap();
    let mut spec = InstallPixiEnvironmentSpec::new(records, prefix);
    spec.build_environment = build_env.clone();
    spec.force_reinstall = HashSet::from_iter([PackageName::new_unchecked("package-b")]);

    dispatcher
        .engine()
        .with_ctx(async |ctx| ctx.install_pixi_environment(spec).await)
        .await
        .unwrap()
        .unwrap();

    let events = observer.build_events();
    assert!(
        events.contains(&BackendEvent::CondaBuildV1Called),
        "force_reinstall on the compute-ctx install path should rebuild \
         the source package; got events: {events:?}",
    );
}

#[tokio::test]
pub async fn pin_and_checkout_url_reuses_cached_checkout() {
    let tempdir = tempfile::tempdir().unwrap();
    let cache_dirs = CacheDirs::new(to_abs_dir(tempdir.path().join("pixi-cache")));
    let url_cache_root = cache_dirs.url();

    let sha = dummy_sha();
    let checkout_dir = prepare_cached_checkout(url_cache_root.as_std_path(), sha);

    let engine = url_test_engine(cache_dirs, None, true, None);

    // Since we have the same expected hash we expect to return existing archive.
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

#[tokio::test]
pub async fn pin_and_checkout_url_reports_sha_mismatch_from_concurrent_request() {
    let tempdir = tempfile::tempdir().unwrap();
    let cache_dirs = CacheDirs::new(to_abs_dir(tempdir.path().join("pixi-cache")));
    let archive = tempfile::tempdir().unwrap();
    let url = file_url_for_test(&archive, "archive.zip");

    let engine = url_test_engine(cache_dirs, None, false, None);

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

#[tokio::test]
pub async fn pin_and_checkout_url_validates_cached_results() {
    let tempdir = tempfile::tempdir().unwrap();
    let cache_dirs = CacheDirs::new(to_abs_dir(tempdir.path().join("pixi-cache")));
    let archive = tempfile::tempdir().unwrap();
    let url = file_url_for_test(&archive, "archive.zip");

    let engine = url_test_engine(cache_dirs, None, true, None);

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

/// Tests that a package is NOT rebuilt across sessions when no source files have changed.
///
/// This test simulates a program restart by dropping and recreating the dispatcher,
/// and verifies that the cache is properly reused (CacheStatus::UpToDate).
#[tokio::test]
pub async fn test_package_not_rebuilt_across_sessions_when_no_files_changed() {
    let root_dir = workspaces_dir().join("host-dependency");
    let tempdir = tempfile::tempdir().unwrap();
    let (tool_platform, tool_virtual_packages) = tool_platform();
    let build_env = BuildEnvironment::simple(tool_platform, tool_virtual_packages.clone());

    let build_command_dispatcher = || {
        CommandDispatcher::builder()
            .with_root_dir(to_abs_dir(root_dir.clone()))
            .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
            .with_executor(Executor::Serial)
            .with_tool_platform(tool_platform, tool_virtual_packages.clone())
            .with_backend_overrides(BackendOverride::from_memory(
                PassthroughBackend::instantiator(),
            ))
    };

    // First session: solve and install
    let dispatcher = build_command_dispatcher().finish();

    let records = run_pixi_solve(
        &dispatcher,
        SolvePixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([
                (
                    "package-a".parse().unwrap(),
                    PathSpec::new("package-a").into(),
                ),
                (
                    "package-c".parse().unwrap(),
                    PathSpec::new("package-c").into(),
                ),
            ]),
            env_ref: env_ref_of(vec![], build_env.clone()),
            ..empty_pixi_env_spec()
        },
    )
    .await
    .map_err(|e| format_diagnostic(&e))
    .expect("solve should succeed");

    let prefix = Prefix::create(tempdir.path().join("prefix")).unwrap();
    dispatcher
        .install_pixi_environment(InstallPixiEnvironmentSpec {
            build_environment: build_env.clone(),
            ..InstallPixiEnvironmentSpec::new(records.clone(), prefix.clone())
        })
        .await
        .map_err(|e| format_diagnostic(&e))
        .expect("install should succeed");

    // Drop dispatcher to simulate program restart
    drop(dispatcher);

    // Second session: reinstall WITHOUT modifying any files
    let (reporter, events) = EventReporter::new();
    let dispatcher = build_command_dispatcher().with_reporter(reporter).finish();

    dispatcher
        .install_pixi_environment(InstallPixiEnvironmentSpec {
            build_environment: build_env.clone(),
            ..InstallPixiEnvironmentSpec::new(records, prefix)
        })
        .await
        .map_err(|e| format_diagnostic(&e))
        .expect("reinstall should succeed");

    let rebuild_packages: Vec<_> = events
        .take()
        .iter()
        .filter_map(|event| match event {
            event_reporter::Event::BackendSourceBuildQueued { package, .. } => {
                Some(package.clone())
            }
            _ => None,
        })
        .collect();

    assert!(
        rebuild_packages.is_empty(),
        "No packages should be rebuilt across sessions when no files changed, but got: {rebuild_packages:?}"
    );
}

/// Tests that a package IS rebuilt across sessions when a source file is modified.
///
/// This test simulates a program restart by dropping and recreating the dispatcher,
/// and verifies that file changes are detected and trigger a rebuild.
#[tokio::test]
pub async fn test_package_rebuilt_across_sessions_when_source_file_modified() {
    // Copy workspace to temp directory so we can modify files without affecting other tests
    let source_dir = workspaces_dir().join("host-dependency");
    let tempdir = tempfile::tempdir().unwrap();
    let root_dir = tempdir.path().join("workspace");
    copy_dir_recursive(&source_dir, &root_dir).unwrap();

    let (tool_platform, tool_virtual_packages) = tool_platform();
    let build_env = BuildEnvironment::simple(tool_platform, tool_virtual_packages.clone());

    let build_command_dispatcher = || {
        CommandDispatcher::builder()
            .with_root_dir(to_abs_dir(root_dir.clone()))
            .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
            .with_executor(Executor::Serial)
            .with_tool_platform(tool_platform, tool_virtual_packages.clone())
            .with_backend_overrides(BackendOverride::from_memory(
                PassthroughBackend::instantiator(),
            ))
    };

    // First session: solve and install
    let dispatcher = build_command_dispatcher().finish();

    let records = run_pixi_solve(
        &dispatcher,
        SolvePixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([
                (
                    "package-a".parse().unwrap(),
                    PathSpec::new("package-a").into(),
                ),
                (
                    "package-c".parse().unwrap(),
                    PathSpec::new("package-c").into(),
                ),
            ]),
            env_ref: env_ref_of(vec![], build_env.clone()),
            ..empty_pixi_env_spec()
        },
    )
    .await
    .map_err(|e| format_diagnostic(&e))
    .expect("solve should succeed");

    let prefix = Prefix::create(tempdir.path().join("prefix")).unwrap();
    dispatcher
        .install_pixi_environment(InstallPixiEnvironmentSpec {
            build_environment: build_env.clone(),
            ..InstallPixiEnvironmentSpec::new(records.clone(), prefix.clone())
        })
        .await
        .map_err(|e| format_diagnostic(&e))
        .expect("install should succeed");

    // Drop dispatcher to simulate program restart
    drop(dispatcher);

    // Create a file that matches package-b's build glob pattern ("TOUCH*")
    fs_err::write(root_dir.join("package-b/TOUCH_FILE"), "trigger rebuild").unwrap();

    // Second session: reinstall after file modification
    let (reporter, events) = EventReporter::new();
    let dispatcher = build_command_dispatcher().with_reporter(reporter).finish();

    dispatcher
        .install_pixi_environment(InstallPixiEnvironmentSpec {
            build_environment: build_env.clone(),
            ..InstallPixiEnvironmentSpec::new(records, prefix)
        })
        .await
        .map_err(|e| format_diagnostic(&e))
        .expect("reinstall should succeed");

    let rebuild_packages: Vec<_> = events
        .take()
        .iter()
        .filter_map(|event| match event {
            event_reporter::Event::BackendSourceBuildQueued { package, .. } => {
                Some(package.clone())
            }
            _ => None,
        })
        .collect();

    assert_eq!(
        rebuild_packages,
        vec!["package-b"],
        "Only package-b should be rebuilt after source file modification"
    );
}

/// Tests that modifying a source file triggers a rebuild of the package.
///
/// This is a focused test that verifies only the file-change detection behavior,
/// without testing dependency chains or force rebuild flags.
#[tokio::test]
pub async fn test_package_rebuilt_when_source_file_modified() {
    // Copy workspace to temp directory so we can modify files without affecting other tests
    let source_dir = workspaces_dir().join("host-dependency");
    let tempdir = tempfile::tempdir().unwrap();
    let root_dir = tempdir.path().join("workspace");
    copy_dir_recursive(&source_dir, &root_dir).unwrap();
    let (tool_platform, tool_virtual_packages) = tool_platform();
    let build_env = BuildEnvironment::simple(tool_platform, tool_virtual_packages.clone());

    let build_command_dispatcher = || {
        CommandDispatcher::builder()
            .with_root_dir(to_abs_dir(root_dir.clone()))
            .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
            .with_executor(Executor::Serial)
            .with_tool_platform(tool_platform, tool_virtual_packages.clone())
            .with_backend_overrides(BackendOverride::from_memory(
                PassthroughBackend::instantiator(),
            ))
    };

    // First pass: build and install package-b
    let dispatcher = build_command_dispatcher().finish();

    let records = run_pixi_solve(
        &dispatcher,
        SolvePixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([(
                "package-b".parse().unwrap(),
                PathSpec::new("package-b").into(),
            )]),
            env_ref: env_ref_of(vec![], build_env.clone()),
            ..empty_pixi_env_spec()
        },
    )
    .await
    .map_err(|e| format_diagnostic(&e))
    .expect("solve should succeed");

    let prefix = Prefix::create(tempdir.path().join("prefix")).unwrap();
    dispatcher
        .install_pixi_environment(InstallPixiEnvironmentSpec {
            build_environment: build_env.clone(),
            ..InstallPixiEnvironmentSpec::new(records.clone(), prefix.clone())
        })
        .await
        .map_err(|e| format_diagnostic(&e))
        .expect("install should succeed");

    // Drop dispatcher to flush caches (simulating program restart)
    drop(dispatcher);

    // Create a file that matches package-b's build glob pattern ("TOUCH*")
    let _touch_file = tempfile::Builder::new()
        .prefix("TOUCH")
        .tempfile_in(root_dir.join("package-b"))
        .unwrap();

    // Second pass: reinstall with new dispatcher, expect rebuild
    let (reporter, events) = EventReporter::new();
    let dispatcher = build_command_dispatcher().with_reporter(reporter).finish();

    dispatcher
        .install_pixi_environment(InstallPixiEnvironmentSpec {
            build_environment: build_env.clone(),
            ..InstallPixiEnvironmentSpec::new(records, prefix)
        })
        .await
        .map_err(|e| format_diagnostic(&e))
        .expect("reinstall should succeed");

    let rebuild_packages: Vec<_> = events
        .take()
        .iter()
        .filter_map(|event| match event {
            event_reporter::Event::BackendSourceBuildQueued { package, .. } => {
                Some(package.clone())
            }
            _ => None,
        })
        .collect();

    assert_eq!(
        rebuild_packages,
        vec!["package-b"],
        "Package should be rebuilt when source file is modified"
    );
}

/// Tests that a package is NOT rebuilt when no source files have changed.
///
/// This is a focused test that verifies cache reuse behavior when files are unchanged
/// within the same dispatcher session (CacheStatus::New is reused).
#[tokio::test]
pub async fn test_package_not_rebuilt_when_no_files_changed() {
    let root_dir = workspaces_dir().join("host-dependency");
    let tempdir = tempfile::tempdir().unwrap();
    let (tool_platform, tool_virtual_packages) = tool_platform();
    let build_env = BuildEnvironment::simple(tool_platform, tool_virtual_packages.clone());

    let (reporter, events) = EventReporter::new();
    let dispatcher = CommandDispatcher::builder()
        .with_root_dir(to_abs_dir(root_dir.clone()))
        .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
        .with_executor(Executor::Serial)
        .with_tool_platform(tool_platform, tool_virtual_packages.clone())
        .with_backend_overrides(BackendOverride::from_memory(
            PassthroughBackend::instantiator(),
        ))
        .with_reporter(reporter)
        .finish();

    // First pass: build and install package-b
    let records = run_pixi_solve(
        &dispatcher,
        SolvePixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([(
                "package-b".parse().unwrap(),
                PathSpec::new("package-b").into(),
            )]),
            env_ref: env_ref_of(vec![], build_env.clone()),
            ..empty_pixi_env_spec()
        },
    )
    .await
    .map_err(|e| format_diagnostic(&e))
    .expect("solve should succeed");

    let prefix = Prefix::create(tempdir.path().join("prefix")).unwrap();
    dispatcher
        .install_pixi_environment(InstallPixiEnvironmentSpec {
            build_environment: build_env.clone(),
            ..InstallPixiEnvironmentSpec::new(records.clone(), prefix.clone())
        })
        .await
        .map_err(|e| format_diagnostic(&e))
        .expect("install should succeed");

    // Count first build events
    let first_build_count: usize = events
        .take()
        .iter()
        .filter(|event| {
            matches!(
                event,
                event_reporter::Event::BackendSourceBuildQueued { .. }
            )
        })
        .count();

    assert_eq!(
        first_build_count, 1,
        "First install should build package-b once"
    );

    // Second pass: reinstall WITHOUT modifying any files (same dispatcher session)
    dispatcher
        .install_pixi_environment(InstallPixiEnvironmentSpec {
            build_environment: build_env.clone(),
            ..InstallPixiEnvironmentSpec::new(records, prefix)
        })
        .await
        .map_err(|e| format_diagnostic(&e))
        .expect("reinstall should succeed");

    let rebuild_packages: Vec<_> = events
        .take()
        .iter()
        .filter_map(|event| match event {
            event_reporter::Event::BackendSourceBuildQueued { package, .. } => {
                Some(package.clone())
            }
            _ => None,
        })
        .collect();

    assert!(
        rebuild_packages.is_empty(),
        "No packages should be rebuilt when no files changed, but got: {rebuild_packages:?}"
    );
}

/// Tests that metadata is NOT re-fetched when no source files have changed.
///
/// This is a focused test that verifies metadata cache reuse behavior.
#[tokio::test]
pub async fn test_metadata_not_refetched_when_no_files_changed() {
    use pixi_command_dispatcher::{BuildBackendMetadataSpec, DevSourceMetadataSpec};
    use pixi_record::PinnedPathSpec;

    let root_dir = workspaces_dir().join("dev-sources");
    let tempdir = tempfile::tempdir().unwrap();
    let (tool_platform, tool_virtual_packages) = tool_platform();

    let build_command_dispatcher = || {
        CommandDispatcher::builder()
            .with_root_dir(to_abs_dir(root_dir.clone()))
            .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
            .with_executor(Executor::Serial)
            .with_tool_platform(tool_platform, tool_virtual_packages.clone())
            .with_backend_overrides(BackendOverride::from_memory(
                PassthroughBackend::instantiator(),
            ))
    };

    let pinned_source: PinnedSourceSpec = PinnedPathSpec {
        path: "test-package".into(),
    }
    .into();

    let spec = DevSourceMetadataSpec {
        package_name: PackageName::new_unchecked("test-package"),
        backend_metadata: BuildBackendMetadataSpec {
            manifest_source: pinned_source,
            preferred_build_source: None,
            env_ref: env_ref_of(
                vec![],
                BuildEnvironment::simple(tool_platform, tool_virtual_packages.clone()),
            ),
        },
    };

    // First metadata request
    let (reporter, events) = EventReporter::new();
    let dispatcher = build_command_dispatcher().with_reporter(reporter).finish();

    dispatcher
        .dev_source_metadata(spec.clone())
        .await
        .map_err(|e| format_diagnostic(&e))
        .expect("first metadata request should succeed");

    let first_metadata_requests: usize = events
        .take()
        .iter()
        .filter(|event| {
            matches!(
                event,
                event_reporter::Event::BuildBackendMetadataQueued { .. }
            )
        })
        .count();

    assert_eq!(
        first_metadata_requests, 1,
        "First request should fetch metadata once"
    );

    // Second metadata request (same dispatcher, no file changes)
    dispatcher.clear_reporter().await;

    dispatcher
        .dev_source_metadata(spec)
        .await
        .map_err(|e| format_diagnostic(&e))
        .expect("second metadata request should succeed");

    let second_metadata_requests: usize = events
        .take()
        .iter()
        .filter(|event| {
            matches!(
                event,
                event_reporter::Event::BuildBackendMetadataQueued { .. }
            )
        })
        .count();

    assert_eq!(
        second_metadata_requests, 0,
        "Second request should use cached metadata, no backend call expected"
    );
}

/// Tests that metadata IS re-fetched when a source file is modified.
///
/// This is a focused test that verifies metadata cache invalidation on file changes.
#[tokio::test]
pub async fn test_metadata_refetched_when_source_file_modified() {
    use pixi_command_dispatcher::{BuildBackendMetadataSpec, DevSourceMetadataSpec};
    use pixi_record::PinnedPathSpec;

    // Copy workspace to temp directory so we can modify files without affecting other tests
    let source_dir = workspaces_dir().join("host-dependency");
    let tempdir = tempfile::tempdir().unwrap();
    let root_dir = tempdir.path().join("workspace");
    copy_dir_recursive(&source_dir, &root_dir).unwrap();
    let (tool_platform, tool_virtual_packages) = tool_platform();

    let build_command_dispatcher = || {
        CommandDispatcher::builder()
            .with_root_dir(to_abs_dir(root_dir.clone()))
            .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
            .with_executor(Executor::Serial)
            .with_tool_platform(tool_platform, tool_virtual_packages.clone())
            .with_backend_overrides(BackendOverride::from_memory(
                PassthroughBackend::instantiator(),
            ))
    };

    let pinned_source: PinnedSourceSpec = PinnedPathSpec {
        path: "package-b".into(),
    }
    .into();

    let spec = DevSourceMetadataSpec {
        package_name: PackageName::new_unchecked("package-b"),
        backend_metadata: BuildBackendMetadataSpec {
            manifest_source: pinned_source,
            preferred_build_source: None,
            env_ref: env_ref_of(
                vec![],
                BuildEnvironment::simple(tool_platform, tool_virtual_packages.clone()),
            ),
        },
    };

    // First metadata request
    let dispatcher = build_command_dispatcher().finish();

    dispatcher
        .dev_source_metadata(spec.clone())
        .await
        .map_err(|e| format_diagnostic(&e))
        .expect("first metadata request should succeed");

    // Drop dispatcher to flush caches (simulating program restart)
    drop(dispatcher);

    // Create a file that matches package-b's build glob pattern ("TOUCH*")
    let _touch_file = tempfile::Builder::new()
        .prefix("TOUCH")
        .tempfile_in(root_dir.join("package-b"))
        .unwrap();

    // Second metadata request after file modification
    let (reporter, events) = EventReporter::new();
    let dispatcher = build_command_dispatcher().with_reporter(reporter).finish();

    dispatcher
        .dev_source_metadata(spec)
        .await
        .map_err(|e| format_diagnostic(&e))
        .expect("second metadata request should succeed");

    let metadata_requests: usize = events
        .take()
        .iter()
        .filter(|event| {
            matches!(
                event,
                event_reporter::Event::BuildBackendMetadataQueued { .. }
            )
        })
        .count();

    assert_eq!(
        metadata_requests, 1,
        "Metadata should be re-fetched after source file modification"
    );
}

/// Verifies that the compute engine is wired into the CommandDispatcher and
/// that extension traits on DataStore provide access to shared resources.
#[tokio::test]
pub async fn compute_engine_wired_into_dispatcher() {
    use pixi_command_dispatcher::compute_data::{
        HasCacheDirs, HasDownloadClient, HasGateway, HasGitResolver, HasUrlResolver,
    };
    use pixi_compute_engine::{ComputeCtx, Key};
    use std::fmt;

    // A trivial Key that reads Gateway from global_data to prove the wiring works.
    #[derive(Clone, Debug, Hash, PartialEq, Eq)]
    struct ProbeKey;

    impl fmt::Display for ProbeKey {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "ProbeKey")
        }
    }

    impl Key for ProbeKey {
        type Value = bool;
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            let data = ctx.global_data();
            // Each trait accessor must succeed without panicking.
            let _ = data.gateway();
            let _ = data.git_resolver();
            let _ = data.url_resolver();
            let _ = data.download_client();
            let _ = data.cache_dirs();
            true
        }
    }

    let dispatcher = CommandDispatcher::builder()
        .with_cache_dirs(default_cache_dirs())
        .finish();

    // Run a Key through the engine; if global data is missing this panics.
    let result = dispatcher.engine().compute(&ProbeKey).await.unwrap();
    assert!(result, "ProbeKey should return true after reading all data");
}

/// Verifies that a compute-engine-backed URL checkout emits the full
/// reporter lifecycle in order. The reporter is discovered through
/// `DataStore`; the `CheckoutUrl` Key fires `on_queued`, then acquires
/// the semaphore, then fires `on_started`, then fetches, then
/// `on_finished` via a drop-guard.
#[tokio::test]
pub async fn reporter_url_checkout_lifecycle() {
    let tempdir = tempfile::tempdir().unwrap();
    let cache_dirs = CacheDirs::new(to_abs_dir(tempdir.path().join("pixi-cache")));
    let archive = tempfile::tempdir().unwrap();
    let url = file_url_for_test(&archive, "archive.zip");

    let (reporter, events) = EventReporter::new();
    let engine = url_test_engine(cache_dirs, Some(Arc::new(reporter)), true, None);

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
        .expect("url checkout should succeed");

    let events = events.take();
    let url_events: Vec<_> = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                event_reporter::Event::UrlCheckoutQueued { .. }
                    | event_reporter::Event::UrlCheckoutStarted { .. }
                    | event_reporter::Event::UrlCheckoutFinished { .. }
            )
        })
        .collect();

    assert_eq!(
        url_events.len(),
        3,
        "expected 3 url lifecycle events, got: {url_events:#?}"
    );
    assert!(matches!(
        url_events[0],
        event_reporter::Event::UrlCheckoutQueued { context: None, .. }
    ));
    assert!(matches!(
        url_events[1],
        event_reporter::Event::UrlCheckoutStarted { .. }
    ));
    assert!(matches!(
        url_events[2],
        event_reporter::Event::UrlCheckoutFinished { .. }
    ));
}

/// Two concurrent `checkout_url` calls for the same URL dedup to a
/// single compute, so the reporter lifecycle fires exactly once.
#[tokio::test]
pub async fn reporter_url_checkout_dedup() {
    let tempdir = tempfile::tempdir().unwrap();
    let cache_dirs = CacheDirs::new(to_abs_dir(tempdir.path().join("pixi-cache")));
    let archive = tempfile::tempdir().unwrap();
    let url = file_url_for_test(&archive, "archive.zip");

    let (reporter, events) = EventReporter::new();
    let engine = url_test_engine(cache_dirs, Some(Arc::new(reporter)), false, None);

    let spec = UrlSpec {
        url: url.clone(),
        md5: None,
        sha256: None,
        subdirectory: Subdirectory::default(),
    };

    let spec_a = spec.clone();
    let spec_b = spec.clone();
    let (a, b) = tokio::join!(
        engine.with_ctx(async |ctx| ctx.pin_and_checkout_url(spec_a).await),
        engine.with_ctx(async |ctx| ctx.pin_and_checkout_url(spec_b).await),
    );
    a.expect("engine scope")
        .expect("first checkout should succeed");
    b.expect("engine scope")
        .expect("second checkout should succeed");

    let events = events.take();
    let queued = events
        .iter()
        .filter(|e| matches!(e, event_reporter::Event::UrlCheckoutQueued { .. }))
        .count();
    let started = events
        .iter()
        .filter(|e| matches!(e, event_reporter::Event::UrlCheckoutStarted { .. }))
        .count();
    let finished = events
        .iter()
        .filter(|e| matches!(e, event_reporter::Event::UrlCheckoutFinished { .. }))
        .count();
    assert_eq!(
        (queued, started, finished),
        (1, 1, 1),
        "deduped URL checkout should fire the lifecycle exactly once, \
         got queued={queued} started={started} finished={finished}"
    );
}

/// With `max_concurrent_url_checkouts = 1`, `on_started` for each
/// distinct URL is serialized behind the previous URL's `on_finished`.
/// The semaphore lives in `DataStore` and is acquired between
/// `on_queued` and `on_started` inside the `CheckoutUrl` Key.
#[tokio::test]
pub async fn semaphore_serializes_concurrent_url_checkouts() {
    let tempdir = tempfile::tempdir().unwrap();
    let cache_dirs = CacheDirs::new(to_abs_dir(tempdir.path().join("pixi-cache")));
    let archive = tempfile::tempdir().unwrap();
    let url_a = file_url_for_test(&archive, "a.zip");
    let url_b = file_url_for_test(&archive, "b.zip");
    let url_c = file_url_for_test(&archive, "c.zip");

    let (reporter, events) = EventReporter::new();
    let engine = url_test_engine(cache_dirs, Some(Arc::new(reporter)), false, Some(1));

    let mk = |url: Url| UrlSpec {
        url,
        md5: None,
        sha256: None,
        subdirectory: Subdirectory::default(),
    };

    let spec_a = mk(url_a);
    let spec_b = mk(url_b);
    let spec_c = mk(url_c);
    let (a, b, c) = tokio::join!(
        engine.with_ctx(async |ctx| ctx.pin_and_checkout_url(spec_a).await),
        engine.with_ctx(async |ctx| ctx.pin_and_checkout_url(spec_b).await),
        engine.with_ctx(async |ctx| ctx.pin_and_checkout_url(spec_c).await),
    );
    a.expect("a engine scope").expect("a should succeed");
    b.expect("b engine scope").expect("b should succeed");
    c.expect("c engine scope").expect("c should succeed");

    // Serialization invariant: across the entire recorded event stream,
    // between any `UrlCheckoutStarted(id)` and its matching
    // `UrlCheckoutFinished(id)`, no other `UrlCheckoutStarted` appears.
    let events = events.take();
    let mut in_flight: Option<pixi_command_dispatcher::reporter::UrlCheckoutId> = None;
    for ev in &events {
        match ev {
            event_reporter::Event::UrlCheckoutStarted { id } => {
                assert!(
                    in_flight.is_none(),
                    "on_started for {id:?} fired while {in_flight:?} was still in flight; \
                     semaphore did not serialize"
                );
                in_flight = Some(*id);
            }
            event_reporter::Event::UrlCheckoutFinished { id } => {
                assert_eq!(
                    in_flight,
                    Some(*id),
                    "on_finished for {id:?} without a matching on_started"
                );
                in_flight = None;
            }
            _ => {}
        }
    }
    assert!(in_flight.is_none(), "a checkout was still in flight at end");
}

/// `SolvePixiEnvironmentSpec::installed` is handed to the solver as a
/// locked-package set: when the binary spec admits multiple versions,
/// the solver must keep the installed version rather than pick the
/// latest.
///
/// The fixture channel has `package` at `0.1.0` and `0.2.0`. A fresh
/// solve against `package = "*"` picks `0.2.0` (the highest match).
/// A second solve against the same spec but with an installed hint
/// pinning `0.1.0` must keep `0.1.0`. Both branches are asserted, so
/// the test proves the hint actually influences the choice.
#[tokio::test]
pub async fn test_installed_pins_binary_version() {
    use rattler_conda_types::VersionSpec;

    let channel_dir =
        cargo_workspace_dir().join("tests/data/channels/channels/multiple_versions_channel_1");
    let channel_url: ChannelUrl = Url::from_directory_path(&channel_dir).unwrap().into();
    let (tool_platform, tool_virtual_packages) = tool_platform();
    let tempdir = tempfile::tempdir().unwrap();
    let dispatcher = CommandDispatcher::builder()
        .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
        .with_executor(Executor::Serial)
        .with_tool_platform(tool_platform, tool_virtual_packages.clone())
        .finish();
    let build_env = BuildEnvironment::simple(tool_platform, tool_virtual_packages);
    let env_ref = env_ref_of(vec![channel_url], build_env);

    let make_spec = |version: VersionSpec, installed: Vec<pixi_record::UnresolvedPixiRecord>| {
        SolvePixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([(
                "package".parse().unwrap(),
                PixiSpec::Version(version),
            )]),
            installed: std::sync::Arc::from(installed),
            env_ref: env_ref.clone(),
            ..empty_pixi_env_spec()
        }
    };

    // Fresh solve with `*`: picks the highest match (0.2.0).
    let fresh = run_pixi_solve(&dispatcher, make_spec(VersionSpec::Any, Vec::new()))
        .await
        .unwrap();
    assert_eq!(
        fresh[0].package_record().version.as_str(),
        "0.2.0",
        "fresh solve against `*` should pick the highest available (0.2.0)"
    );

    // Narrow solve to acquire a concrete 0.1.0 record for the hint.
    let v010 = run_pixi_solve(
        &dispatcher,
        make_spec("==0.1.0".parse::<VersionSpec>().unwrap(), Vec::new()),
    )
    .await
    .unwrap();
    assert_eq!(v010[0].package_record().version.as_str(), "0.1.0");

    // Stability: `*` again, but installed pins 0.1.0. Solver must keep 0.1.0.
    let pinned = run_pixi_solve(
        &dispatcher,
        make_spec(
            VersionSpec::Any,
            v010.iter().cloned().map(to_unresolved).collect(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        pinned[0].package_record().version.as_str(),
        "0.1.0",
        "with installed = [0.1.0], `*` must keep 0.1.0 instead of picking 0.2.0"
    );
}

/// When `SolvePixiEnvironmentSpec::installed` carries a previously-
/// resolved source record, the per-package `host_packages` set on that
/// record must flow into the nested Host-env solve as its own
/// `installed` hint. Without the hint the nested solve picks the
/// highest-admitted version; with the hint it must keep the recorded
/// version.
///
/// `foo` is a source package with a single host-dep `package = "*"`
/// against the fixture channel (versions 0.1.0, 0.2.0). Fresh solve
/// resolves foo and picks `package 0.2.0` into `foo.host_packages`.
/// A second solve with an altered prior-record (`host_packages` set
/// to `package 0.1.0`) must leave the nested solve pinned to 0.1.0.
#[tokio::test]
pub async fn test_installed_host_packages_pin_nested_solve() {
    use rattler_conda_types::VersionSpec;

    let channel_dir =
        cargo_workspace_dir().join("tests/data/channels/channels/multiple_versions_channel_1");
    let channel_url: ChannelUrl = Url::from_directory_path(&channel_dir).unwrap().into();
    let (tool_platform, tool_virtual_packages) = tool_platform();
    let root_dir = workspaces_dir().join("host-dep-binary");
    let tempdir = tempfile::tempdir().unwrap();
    let dispatcher = CommandDispatcher::builder()
        .with_root_dir(to_abs_dir(root_dir))
        .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
        .with_executor(Executor::Serial)
        .with_tool_platform(tool_platform, tool_virtual_packages.clone())
        .with_backend_overrides(BackendOverride::from_memory(
            PassthroughBackend::instantiator(),
        ))
        .finish();
    let build_env = BuildEnvironment::simple(tool_platform, tool_virtual_packages);
    let env_ref = env_ref_of(vec![channel_url], build_env);

    let make_spec = |installed: Vec<pixi_record::UnresolvedPixiRecord>| {
        let (installed, installed_source_hints) = installed_with_hints(installed);
        SolvePixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([(
                "foo".parse().unwrap(),
                PathSpec { path: "foo".into() }.into(),
            )]),
            installed,
            installed_source_hints,
            env_ref: env_ref.clone(),
            ..empty_pixi_env_spec()
        }
    };

    // Helper: find foo's resolved source record, pluck the `package`
    // binary out of its host_packages.
    let host_package_version = |records: &[pixi_record::PixiRecord]| -> String {
        let foo = records
            .iter()
            .find_map(|r| r.as_source().filter(|s| s.name().as_normalized() == "foo"))
            .expect("foo source record in solve output");
        let pkg = foo
            .host_packages
            .iter()
            .find(|u| u.name().as_normalized() == "package")
            .expect("`package` in foo.host_packages");
        pkg.package_record()
            .expect("resolved host_packages entry")
            .version
            .as_str()
            .to_string()
    };

    // Fresh: foo's host env picks the highest (0.2.0).
    let fresh = run_pixi_solve(&dispatcher, make_spec(Vec::new()))
        .await
        .unwrap();
    assert_eq!(
        host_package_version(&fresh),
        "0.2.0",
        "fresh Host solve should pick the highest match (0.2.0)"
    );

    // Build a prior-record for foo whose host_packages pins
    // `package 0.1.0`. Re-use the top-level 0.1.0 record from a
    // narrow binary solve so we have a real RepoDataRecord with the
    // correct url/sha.
    let v010_records = run_pixi_solve(
        &dispatcher,
        SolvePixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([(
                "package".parse().unwrap(),
                PixiSpec::Version("==0.1.0".parse::<VersionSpec>().unwrap()),
            )]),
            installed: std::sync::Arc::from([]),
            env_ref: env_ref.clone(),
            ..empty_pixi_env_spec()
        },
    )
    .await
    .unwrap();
    let package_0_1_0: pixi_record::UnresolvedPixiRecord = v010_records
        .into_iter()
        .find(|r| r.name().as_normalized() == "package")
        .expect("package 0.1.0 binary record")
        .into();

    // Splice the hint into foo's fresh record: clone fresh, replace
    // foo's host_packages with just [package 0.1.0].
    let mut installed: Vec<pixi_record::UnresolvedPixiRecord> =
        fresh.iter().cloned().map(to_unresolved).collect();
    for entry in installed.iter_mut() {
        if let pixi_record::UnresolvedPixiRecord::Source(arc) = entry
            && arc.name().as_normalized() == "foo"
        {
            let mut inner = (**arc).clone();
            inner.host_packages = vec![package_0_1_0.clone()];
            *arc = std::sync::Arc::new(inner);
        }
    }

    // Stability: foo's Host env solve must now keep 0.1.0.
    let pinned = run_pixi_solve(&dispatcher, make_spec(installed))
        .await
        .unwrap();
    assert_eq!(
        host_package_version(&pinned),
        "0.1.0",
        "with foo.host_packages = [package 0.1.0], nested Host solve must keep 0.1.0"
    );
}

/// An environment holds at most one variant of any given package, so
/// the walk's installed-hint lookup keys by package name (not by
/// `(name, variants)`). That means when the backend emits multiple
/// variants for the same source package, every variant's nested
/// build/host solve inherits the same hint from the one previously-
/// recorded record.
///
/// Setup mirrors Test 2 but enables a variant grid `package: [0.1.0,
/// 0.2.0]` (picked up from foo's run-dep). The backend emits 2
/// variants of foo; each variant's Host env still resolves the non-
/// star host-dep `package >=0.1` via the solver. Without the hint,
/// the solver picks 0.2.0. With a hint carrying a previously-recorded
/// `[package 0.1.0]`, every variant's Host solve reuses 0.1.0, so
/// whichever variant `SolveCondaKey` ultimately picks, the final
/// record's host_packages reflects the hint.
#[tokio::test]
pub async fn test_installed_hint_reused_across_variants() {
    use rattler_conda_types::VersionSpec;

    let channel_dir =
        cargo_workspace_dir().join("tests/data/channels/channels/multiple_versions_channel_1");
    let channel_url: ChannelUrl = Url::from_directory_path(&channel_dir).unwrap().into();
    let (tool_platform, tool_virtual_packages) = tool_platform();
    let root_dir = workspaces_dir().join("host-dep-binary");
    let tempdir = tempfile::tempdir().unwrap();
    let dispatcher = CommandDispatcher::builder()
        .with_root_dir(to_abs_dir(root_dir))
        .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
        .with_executor(Executor::Serial)
        .with_tool_platform(tool_platform, tool_virtual_packages.clone())
        .with_backend_overrides(BackendOverride::from_memory(
            PassthroughBackend::instantiator(),
        ))
        .finish();

    // `package` listed with two values → backend fans out to 2
    // variants of foo (one output per variant).
    let mut variant_config = BTreeMap::new();
    variant_config.insert(
        "package".to_string(),
        vec!["0.1.0".to_string().into(), "0.2.0".to_string().into()],
    );

    let env_ref = EnvironmentRef::Ephemeral(EphemeralEnv::new(
        "test",
        EnvironmentSpec {
            channels: vec![channel_url.clone()],
            build_environment: BuildEnvironment::simple(
                tool_platform,
                tool_virtual_packages.clone(),
            ),
            variants: VariantConfig {
                variant_configuration: variant_config,
                variant_files: Vec::new(),
            },
            exclude_newer: None,
            channel_priority: Default::default(),
        },
    ));

    // Channel-only env_ref for the side solve that fetches a 0.1.0
    // binary record to splice into the hint.
    let binary_env_ref = env_ref_of(
        vec![channel_url],
        BuildEnvironment::simple(tool_platform, tool_virtual_packages),
    );

    let make_spec = |installed: Vec<pixi_record::UnresolvedPixiRecord>| {
        let (installed, installed_source_hints) = installed_with_hints(installed);
        SolvePixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([(
                "foo".parse().unwrap(),
                PathSpec { path: "foo".into() }.into(),
            )]),
            installed,
            installed_source_hints,
            env_ref: env_ref.clone(),
            ..empty_pixi_env_spec()
        }
    };

    let host_package_version = |records: &[pixi_record::PixiRecord]| -> String {
        let foo = records
            .iter()
            .find_map(|r| r.as_source().filter(|s| s.name().as_normalized() == "foo"))
            .expect("foo source record in solve output");
        let pkg = foo
            .host_packages
            .iter()
            .find(|u| u.name().as_normalized() == "package")
            .expect("`package` in foo.host_packages");
        pkg.package_record()
            .expect("resolved host_packages entry")
            .version
            .as_str()
            .to_string()
    };

    // Fresh, multi-variant: each variant's Host env picks the highest
    // match for `package >=0.1`, which is 0.2.0.
    let fresh = run_pixi_solve(&dispatcher, make_spec(Vec::new()))
        .await
        .unwrap();
    assert_eq!(
        host_package_version(&fresh),
        "0.2.0",
        "fresh multi-variant Host solve should pick 0.2.0"
    );

    // Acquire a concrete 0.1.0 binary record for the hint.
    let v010_records = run_pixi_solve(
        &dispatcher,
        SolvePixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([(
                "package".parse().unwrap(),
                PixiSpec::Version("==0.1.0".parse::<VersionSpec>().unwrap()),
            )]),
            installed: std::sync::Arc::from([]),
            env_ref: binary_env_ref,
            ..empty_pixi_env_spec()
        },
    )
    .await
    .unwrap();
    let package_0_1_0: pixi_record::UnresolvedPixiRecord = v010_records
        .into_iter()
        .find(|r| r.name().as_normalized() == "package")
        .expect("package 0.1.0 binary record")
        .into();

    // Splice the hint into foo's fresh record. The walk keys by
    // package name, so the same hint flows into every variant's
    // nested Host solve; whichever variant SolveCondaKey finally
    // picks, its host_packages carries the pinned 0.1.0.
    let mut installed: Vec<pixi_record::UnresolvedPixiRecord> =
        fresh.iter().cloned().map(to_unresolved).collect();
    for entry in installed.iter_mut() {
        if let pixi_record::UnresolvedPixiRecord::Source(arc) = entry
            && arc.name().as_normalized() == "foo"
        {
            let mut inner = (**arc).clone();
            inner.host_packages = vec![package_0_1_0.clone()];
            *arc = std::sync::Arc::new(inner);
        }
    }

    let pinned = run_pixi_solve(&dispatcher, make_spec(installed))
        .await
        .unwrap();
    assert_eq!(
        host_package_version(&pinned),
        "0.1.0",
        "installed hint should flow into every variant's Host solve, \
         so the picked variant's host_packages must reflect 0.1.0"
    );
}

/// Duplicate installed source hints for the same source package should
/// be normalized before the solve, so their input order does not affect
/// the nested source resolution result.
#[tokio::test]
pub async fn test_duplicate_installed_source_hints_are_order_independent() {
    use rattler_conda_types::VersionSpec;

    let channel_dir =
        cargo_workspace_dir().join("tests/data/channels/channels/multiple_versions_channel_1");
    let channel_url: ChannelUrl = Url::from_directory_path(&channel_dir).unwrap().into();
    let (tool_platform, tool_virtual_packages) = tool_platform();
    let root_dir = workspaces_dir().join("host-dep-binary");
    let tempdir = tempfile::tempdir().unwrap();
    let dispatcher = CommandDispatcher::builder()
        .with_root_dir(to_abs_dir(root_dir))
        .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
        .with_executor(Executor::Serial)
        .with_tool_platform(tool_platform, tool_virtual_packages.clone())
        .with_backend_overrides(BackendOverride::from_memory(
            PassthroughBackend::instantiator(),
        ))
        .finish();
    let build_env = BuildEnvironment::simple(tool_platform, tool_virtual_packages);
    let env_ref = env_ref_of(vec![channel_url.clone()], build_env);

    let make_spec = |installed: Vec<pixi_record::UnresolvedPixiRecord>| {
        let (installed, installed_source_hints) = installed_with_hints(installed);
        SolvePixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([(
                "foo".parse().unwrap(),
                PathSpec { path: "foo".into() }.into(),
            )]),
            installed,
            installed_source_hints,
            env_ref: env_ref.clone(),
            ..empty_pixi_env_spec()
        }
    };

    let host_package_version = |records: &[pixi_record::PixiRecord]| -> String {
        let foo = records
            .iter()
            .find_map(|r| r.as_source().filter(|s| s.name().as_normalized() == "foo"))
            .expect("foo source record in solve output");
        let pkg = foo
            .host_packages
            .iter()
            .find(|u| u.name().as_normalized() == "package")
            .expect("`package` in foo.host_packages");
        pkg.package_record()
            .expect("resolved host_packages entry")
            .version
            .as_str()
            .to_string()
    };

    let fresh = run_pixi_solve(&dispatcher, make_spec(Vec::new()))
        .await
        .unwrap();
    let foo_source = fresh
        .iter()
        .find_map(|r| r.as_source().filter(|s| s.name().as_normalized() == "foo"))
        .expect("foo source record from fresh solve");

    let package_0_1_0: pixi_record::UnresolvedPixiRecord = run_pixi_solve(
        &dispatcher,
        SolvePixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([(
                "package".parse().unwrap(),
                PixiSpec::Version("==0.1.0".parse::<VersionSpec>().unwrap()),
            )]),
            installed: std::sync::Arc::from([]),
            env_ref: env_ref.clone(),
            ..empty_pixi_env_spec()
        },
    )
    .await
    .unwrap()
    .into_iter()
    .find(|r| r.name().as_normalized() == "package")
    .expect("package 0.1.0 binary record")
    .into();
    let package_0_2_0: pixi_record::UnresolvedPixiRecord = run_pixi_solve(
        &dispatcher,
        SolvePixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([(
                "package".parse().unwrap(),
                PixiSpec::Version("==0.2.0".parse::<VersionSpec>().unwrap()),
            )]),
            installed: std::sync::Arc::from([]),
            env_ref: env_ref.clone(),
            ..empty_pixi_env_spec()
        },
    )
    .await
    .unwrap()
    .into_iter()
    .find(|r| r.name().as_normalized() == "package")
    .expect("package 0.2.0 binary record")
    .into();

    let mut foo_with_0_1_0 = foo_source.clone();
    foo_with_0_1_0.host_packages = vec![package_0_1_0];
    let mut foo_with_0_2_0 = foo_source.clone();
    foo_with_0_2_0.host_packages = vec![package_0_2_0];

    let low_then_high = run_pixi_solve(
        &dispatcher,
        make_spec(vec![
            pixi_record::UnresolvedPixiRecord::Source(Arc::new(foo_with_0_1_0.clone().into())),
            pixi_record::UnresolvedPixiRecord::Source(Arc::new(foo_with_0_2_0.clone().into())),
        ]),
    )
    .await
    .unwrap();

    let high_then_low = run_pixi_solve(
        &dispatcher,
        make_spec(vec![
            pixi_record::UnresolvedPixiRecord::Source(Arc::new(foo_with_0_2_0.into())),
            pixi_record::UnresolvedPixiRecord::Source(Arc::new(foo_with_0_1_0.into())),
        ]),
    )
    .await
    .unwrap();

    assert_eq!(
        host_package_version(&low_then_high),
        host_package_version(&high_then_low),
        "duplicate installed source hints should be normalized before solving"
    );
}

/// When the environment contains both a top-level source package `foo`
/// and another source package `bar` whose host environment also resolves
/// `foo`, lockfile-derived installed hints can currently carry two
/// different locked instances of `foo` in the same environment.
///
/// The solve should normalize those hints so the final environment does
/// not contain divergent `foo` instances depending on where they were
/// reached from.
#[tokio::test]
pub async fn test_top_level_and_nested_source_hints_for_same_package_are_normalized() {
    use rattler_conda_types::VersionSpec;

    let channel_dir =
        cargo_workspace_dir().join("tests/data/channels/channels/multiple_versions_channel_1");
    let channel_url: ChannelUrl = Url::from_directory_path(&channel_dir).unwrap().into();
    let (tool_platform, tool_virtual_packages) = tool_platform();
    let root_dir = tempfile::tempdir().unwrap();
    fs::write(
        root_dir.path().join("pixi.toml"),
        r#"
[workspace]
channels = []
platforms = []
preview = ["pixi-build"]
"#,
    )
    .unwrap();
    fs::create_dir_all(root_dir.path().join("foo")).unwrap();
    fs::create_dir_all(root_dir.path().join("bar")).unwrap();
    fs::write(
        root_dir.path().join("foo").join("pixi.toml"),
        r#"
[package]
name = "foo"
version = "0.1.0"

[package.build]
backend = { name = "in-memory", version = "*" }

[package.host-dependencies]
package = ">=0.1"
"#,
    )
    .unwrap();
    fs::write(
        root_dir.path().join("bar").join("pixi.toml"),
        r#"
[package]
name = "bar"
version = "0.1.0"

[package.build]
backend = { name = "in-memory", version = "*" }

[package.host-dependencies]
foo = { path = "../foo" }
"#,
    )
    .unwrap();

    let scratch = tempfile::tempdir().unwrap();
    let dispatcher = CommandDispatcher::builder()
        .with_root_dir(to_abs_dir(root_dir.path()))
        .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(scratch.path())))
        .with_executor(Executor::Serial)
        .with_tool_platform(tool_platform, tool_virtual_packages.clone())
        .with_backend_overrides(BackendOverride::from_memory(
            PassthroughBackend::instantiator(),
        ))
        .finish();
    let build_env = BuildEnvironment::simple(tool_platform, tool_virtual_packages);
    let env_ref = env_ref_of(vec![channel_url.clone()], build_env);

    let make_spec = |installed: Vec<pixi_record::UnresolvedPixiRecord>| SolvePixiEnvironmentSpec {
        dependencies: DependencyMap::from_iter([
            (
                "foo".parse().unwrap(),
                PathSpec { path: "foo".into() }.into(),
            ),
            (
                "bar".parse().unwrap(),
                PathSpec { path: "bar".into() }.into(),
            ),
        ]),
        installed: std::sync::Arc::from(installed),
        env_ref: env_ref.clone(),
        ..empty_pixi_env_spec()
    };

    let host_package_version = |host_packages: &[pixi_record::UnresolvedPixiRecord]| -> String {
        let pkg = host_packages
            .iter()
            .find(|u| u.name().as_normalized() == "package")
            .expect("`package` in foo.host_packages");
        pkg.package_record()
            .expect("resolved host_packages entry")
            .version
            .as_str()
            .to_string()
    };

    let fresh = run_pixi_solve(&dispatcher, make_spec(Vec::new()))
        .await
        .unwrap();
    let top_level_foo = fresh
        .iter()
        .find_map(|r| r.as_source().filter(|s| s.name().as_normalized() == "foo"))
        .expect("top-level foo source record")
        .clone();
    let top_level_bar = fresh
        .iter()
        .find_map(|r| r.as_source().filter(|s| s.name().as_normalized() == "bar"))
        .expect("top-level bar source record")
        .clone();

    let package_0_1_0: pixi_record::UnresolvedPixiRecord = run_pixi_solve(
        &dispatcher,
        SolvePixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([(
                "package".parse().unwrap(),
                PixiSpec::Version("==0.1.0".parse::<VersionSpec>().unwrap()),
            )]),
            installed: std::sync::Arc::from([]),
            env_ref: env_ref.clone(),
            ..empty_pixi_env_spec()
        },
    )
    .await
    .unwrap()
    .into_iter()
    .find(|r| r.name().as_normalized() == "package")
    .expect("package 0.1.0 binary record")
    .into();
    let package_0_2_0: pixi_record::UnresolvedPixiRecord = run_pixi_solve(
        &dispatcher,
        SolvePixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([(
                "package".parse().unwrap(),
                PixiSpec::Version("==0.2.0".parse::<VersionSpec>().unwrap()),
            )]),
            installed: std::sync::Arc::from([]),
            env_ref: env_ref.clone(),
            ..empty_pixi_env_spec()
        },
    )
    .await
    .unwrap()
    .into_iter()
    .find(|r| r.name().as_normalized() == "package")
    .expect("package 0.2.0 binary record")
    .into();

    let mut foo_with_0_1_0 = top_level_foo.clone();
    foo_with_0_1_0.host_packages = vec![package_0_1_0];

    let mut nested_foo_with_0_2_0 = top_level_foo.clone();
    nested_foo_with_0_2_0.host_packages = vec![package_0_2_0];

    let mut bar_with_nested_foo_0_2_0 = top_level_bar;
    bar_with_nested_foo_0_2_0.host_packages = vec![pixi_record::UnresolvedPixiRecord::Source(
        Arc::new(nested_foo_with_0_2_0.into()),
    )];

    let solved = run_pixi_solve(
        &dispatcher,
        make_spec(vec![
            pixi_record::UnresolvedPixiRecord::Source(Arc::new(foo_with_0_1_0.into())),
            pixi_record::UnresolvedPixiRecord::Source(Arc::new(bar_with_nested_foo_0_2_0.into())),
        ]),
    )
    .await
    .unwrap();

    let solved_top_level_foo = solved
        .iter()
        .find_map(|r| r.as_source().filter(|s| s.name().as_normalized() == "foo"))
        .expect("solved top-level foo");
    let solved_bar = solved
        .iter()
        .find_map(|r| r.as_source().filter(|s| s.name().as_normalized() == "bar"))
        .expect("solved bar");
    let nested_foo = solved_bar
        .host_packages
        .iter()
        .find_map(|r| r.as_source().filter(|s| s.name().as_normalized() == "foo"))
        .expect("nested foo inside bar.host_packages");

    assert_eq!(
        host_package_version(&solved_top_level_foo.host_packages),
        host_package_version(&nested_foo.host_packages),
        "the same source package should not resolve to divergent locked host deps \
         depending on whether it is reached top-level or through another source package"
    );
}

/// Two parallel installs of the same workspace into different prefixes
/// should dedup their `SourceBuildKey` invocations: the source package
/// is built exactly once and both installs share the result.
///
/// Exercises the cross-env deduplication guarantee that the compute
/// engine gives us when two callers request equivalent
/// `SourceBuildSpecV2` inputs. If `Hash` or `Eq` on the spec starts
/// discriminating between equivalent callers, or the engine stops
/// deduping Keys, this test fails.
#[tokio::test]
pub async fn test_source_build_key_dedups_across_parallel_installs() {
    let root_dir = workspaces_dir().join("host-dependency");
    let tempdir = tempfile::tempdir().unwrap();
    let (tool_platform, tool_virtual_packages) = tool_platform();
    let build_env = BuildEnvironment::simple(tool_platform, tool_virtual_packages.clone());

    let (reporter, events) = EventReporter::new();
    let dispatcher = CommandDispatcher::builder()
        .with_root_dir(to_abs_dir(root_dir.clone()))
        .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
        .with_executor(Executor::Serial)
        .with_tool_platform(tool_platform, tool_virtual_packages)
        .with_backend_overrides(BackendOverride::from_memory(
            PassthroughBackend::instantiator(),
        ))
        .with_reporter(reporter)
        .finish();

    let records = run_pixi_solve(
        &dispatcher,
        SolvePixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([(
                "package-b".parse().unwrap(),
                PathSpec::new("package-b").into(),
            )]),
            env_ref: env_ref_of(vec![], build_env.clone()),
            ..empty_pixi_env_spec()
        },
    )
    .await
    .map_err(|e| format_diagnostic(&e))
    .expect("solve should succeed");

    let prefix_a = Prefix::create(tempdir.path().join("prefix-a")).unwrap();
    let prefix_b = Prefix::create(tempdir.path().join("prefix-b")).unwrap();

    let install_a = dispatcher.install_pixi_environment(InstallPixiEnvironmentSpec {
        build_environment: build_env.clone(),
        ..InstallPixiEnvironmentSpec::new(records.clone(), prefix_a)
    });
    let install_b = dispatcher.install_pixi_environment(InstallPixiEnvironmentSpec {
        build_environment: build_env.clone(),
        ..InstallPixiEnvironmentSpec::new(records, prefix_b)
    });
    let (a, b) = tokio::join!(install_a, install_b);
    a.map_err(|e| format_diagnostic(&e))
        .expect("first install should succeed");
    b.map_err(|e| format_diagnostic(&e))
        .expect("second install should succeed");

    let build_count = events
        .take()
        .iter()
        .filter(|event| matches!(event, Event::BackendSourceBuildQueued { .. }))
        .count();

    assert_eq!(
        build_count, 1,
        "two parallel installs of the same source package should trigger \
         exactly one backend build; SourceBuildKey did not dedup"
    );
}

/// Two parallel `InstantiateBackendKey` computes for the same backend
/// spec should resolve to a single backend instantiation. Backends are
/// frequently shared across environments (workspace + per-package
/// build envs), so dedup failure here multiplies backend startup cost
/// across a session.
#[tokio::test]
pub async fn test_instantiate_backend_key_dedups_across_parallel_installs() {
    let root_dir = workspaces_dir().join("host-dependency");
    let tempdir = tempfile::tempdir().unwrap();
    let (tool_platform, tool_virtual_packages) = tool_platform();
    let build_env = BuildEnvironment::simple(tool_platform, tool_virtual_packages.clone());

    let (reporter, events) = EventReporter::new();
    let dispatcher = CommandDispatcher::builder()
        .with_root_dir(to_abs_dir(root_dir.clone()))
        .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
        .with_executor(Executor::Serial)
        .with_tool_platform(tool_platform, tool_virtual_packages)
        .with_backend_overrides(BackendOverride::from_memory(
            PassthroughBackend::instantiator(),
        ))
        .with_reporter(reporter)
        .finish();

    let records = run_pixi_solve(
        &dispatcher,
        SolvePixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([(
                "package-b".parse().unwrap(),
                PathSpec::new("package-b").into(),
            )]),
            env_ref: env_ref_of(vec![], build_env.clone()),
            ..empty_pixi_env_spec()
        },
    )
    .await
    .map_err(|e| format_diagnostic(&e))
    .expect("solve should succeed");

    let prefix_a = Prefix::create(tempdir.path().join("prefix-a")).unwrap();
    let prefix_b = Prefix::create(tempdir.path().join("prefix-b")).unwrap();

    let install_a = dispatcher.install_pixi_environment(InstallPixiEnvironmentSpec {
        build_environment: build_env.clone(),
        ..InstallPixiEnvironmentSpec::new(records.clone(), prefix_a)
    });
    let install_b = dispatcher.install_pixi_environment(InstallPixiEnvironmentSpec {
        build_environment: build_env.clone(),
        ..InstallPixiEnvironmentSpec::new(records, prefix_b)
    });
    let (a, b) = tokio::join!(install_a, install_b);
    a.map_err(|e| format_diagnostic(&e))
        .expect("first install should succeed");
    b.map_err(|e| format_diagnostic(&e))
        .expect("second install should succeed");

    let instantiate_count = events
        .take()
        .iter()
        .filter(|event| matches!(event, Event::InstantiateBackendQueued { .. }))
        .count();

    assert_eq!(
        instantiate_count, 1,
        "two parallel installs sharing the same backend spec should trigger \
         exactly one backend instantiation; InstantiateBackendKey did not dedup"
    );
}

/// Two parallel `SolvePixiEnvironmentKey` computes for the same spec
/// should dedup. Both the outer pixi solve and the inner conda solve
/// fire their reporter lifecycle exactly once.
///
/// Guards against regressions in `Hash` / `Eq` on
/// [`SolvePixiEnvironmentSpec`] or the engine's Key-level dedup.
#[tokio::test]
pub async fn test_solve_pixi_environment_key_dedups_parallel_identical_solves() {
    let root_dir = workspaces_dir().join("host-dependency");
    let tempdir = tempfile::tempdir().unwrap();
    let (tool_platform, tool_virtual_packages) = tool_platform();
    let build_env = BuildEnvironment::simple(tool_platform, tool_virtual_packages.clone());

    let (reporter, events) = EventReporter::new();
    let dispatcher = CommandDispatcher::builder()
        .with_root_dir(to_abs_dir(root_dir.clone()))
        .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
        .with_executor(Executor::Serial)
        .with_tool_platform(tool_platform, tool_virtual_packages)
        .with_backend_overrides(BackendOverride::from_memory(
            PassthroughBackend::instantiator(),
        ))
        .with_reporter(reporter)
        .finish();

    let make_spec = || SolvePixiEnvironmentSpec {
        dependencies: DependencyMap::from_iter([(
            "package-b".parse().unwrap(),
            PathSpec::new("package-b").into(),
        )]),
        env_ref: env_ref_of(vec![], build_env.clone()),
        ..empty_pixi_env_spec()
    };

    let (a, b) = tokio::join!(
        run_pixi_solve(&dispatcher, make_spec()),
        run_pixi_solve(&dispatcher, make_spec()),
    );
    a.map_err(|e| format_diagnostic(&e))
        .expect("first solve should succeed");
    b.map_err(|e| format_diagnostic(&e))
        .expect("second solve should succeed");

    let events = events.take();
    let pixi_solves = events
        .iter()
        .filter(|event| matches!(event, Event::PixiSolveQueued { .. }))
        .count();
    let conda_solves = events
        .iter()
        .filter(|event| matches!(event, Event::CondaSolveQueued { .. }))
        .count();

    assert_eq!(
        pixi_solves, 1,
        "two parallel identical pixi solves should dedup; got {pixi_solves} queue events"
    );
    assert_eq!(
        conda_solves, 1,
        "the inner conda solve should also dedup; got {conda_solves} queue events"
    );
}

/// Two parallel pixi solves that use distinct [`EphemeralEnv`] wrappers
/// (different display names) around equal [`EnvironmentSpec`]s should
/// collapse into one `SolvePixiEnvironmentKey` compute.
///
/// Guards the intentional design in
/// [`EphemeralEnv`](pixi_command_dispatcher::EphemeralEnv): `name` is
/// display-only and excluded from `Hash` / `Eq`, so the key's identity
/// is content-addressed on the underlying spec. A regression that
/// starts hashing the name would fan out to two redundant solves.
#[tokio::test]
pub async fn test_solve_pixi_environment_key_dedups_across_ephemeral_env_names() {
    let root_dir = workspaces_dir().join("host-dependency");
    let tempdir = tempfile::tempdir().unwrap();
    let (tool_platform, tool_virtual_packages) = tool_platform();
    let build_env = BuildEnvironment::simple(tool_platform, tool_virtual_packages.clone());

    let (reporter, events) = EventReporter::new();
    let dispatcher = CommandDispatcher::builder()
        .with_root_dir(to_abs_dir(root_dir.clone()))
        .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
        .with_executor(Executor::Serial)
        .with_tool_platform(tool_platform, tool_virtual_packages)
        .with_backend_overrides(BackendOverride::from_memory(
            PassthroughBackend::instantiator(),
        ))
        .with_reporter(reporter)
        .finish();

    let make_env_ref = |name: &str| {
        EnvironmentRef::Ephemeral(EphemeralEnv::new(
            name,
            EnvironmentSpec {
                channels: Vec::new(),
                build_environment: build_env.clone(),
                variants: VariantConfig::default(),
                exclude_newer: None,
                channel_priority: Default::default(),
            },
        ))
    };
    let base_spec = SolvePixiEnvironmentSpec {
        dependencies: DependencyMap::from_iter([(
            "package-b".parse().unwrap(),
            PathSpec::new("package-b").into(),
        )]),
        env_ref: make_env_ref("env-a"),
        ..empty_pixi_env_spec()
    };
    let spec_a = base_spec.clone();
    let spec_b = SolvePixiEnvironmentSpec {
        env_ref: make_env_ref("env-b"),
        ..base_spec
    };

    let (a, b) = tokio::join!(
        run_pixi_solve(&dispatcher, spec_a),
        run_pixi_solve(&dispatcher, spec_b),
    );
    a.map_err(|e| format_diagnostic(&e))
        .expect("env-a solve should succeed");
    b.map_err(|e| format_diagnostic(&e))
        .expect("env-b solve should succeed");

    let events = events.take();
    let pixi_solves = events
        .iter()
        .filter(|event| matches!(event, Event::PixiSolveQueued { .. }))
        .count();
    let conda_solves = events
        .iter()
        .filter(|event| matches!(event, Event::CondaSolveQueued { .. }))
        .count();

    assert_eq!(
        pixi_solves, 1,
        "two ephemeral envs with equal specs but different names should \
         dedup at SolvePixiEnvironmentKey; got {pixi_solves} queue events"
    );
    assert_eq!(
        conda_solves, 1,
        "dedup at the outer pixi solve should subsume the inner conda \
         solve too; got {conda_solves} queue events"
    );
}
