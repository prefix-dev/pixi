mod event_reporter;
mod event_tree;

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    // ptr,
    str::FromStr,
};

use pixi_path::AbsPathBuf;

use event_reporter::EventReporter;
use fs_err as fs;
use itertools::Itertools;
use pixi_build_backend_passthrough::PassthroughBackend;
use pixi_build_frontend::{BackendOverride, InMemoryOverriddenBackends};
use pixi_command_dispatcher::{
    BuildEnvironment, CacheDirs, CommandDispatcher, CommandDispatcherError, Executor,
    InstallPixiEnvironmentSpec, InstantiateToolEnvironmentSpec, PackageIdentifier,
    PixiEnvironmentSpec, SourceBuildCacheStatusSpec, build::SourceCodeLocation,
};
use pixi_config::default_channel_config;
use pixi_record::{PinnedPathSpec, PinnedSourceSpec};
use pixi_spec::{GitReference, GitSpec, PathSpec, PixiSpec, Subdirectory, UrlSpec};
use pixi_spec_containers::DependencyMap;
use pixi_test_utils::format_diagnostic;
use pixi_url::UrlError;
use rattler_conda_types::{
    ChannelUrl, GenericVirtualPackage, PackageName, Platform, VersionSpec, VersionWithSource,
    prefix::Prefix,
};
use rattler_digest::{Sha256, Sha256Hash, digest::Digest};
use rattler_virtual_packages::{VirtualPackageOverrides, VirtualPackages};
use tempfile::TempDir;
use url::Url;

use crate::{event_reporter::Event, event_tree::EventTree};

/// Converts a PathBuf to AbsPresumedDirPathBuf for tests.
fn to_abs_dir(path: impl Into<PathBuf>) -> pixi_path::AbsPresumedDirPathBuf {
    AbsPathBuf::new(path)
        .expect("path is not absolute")
        .into_assume_dir()
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

    let records = dispatcher
        .solve_pixi_environment(PixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([(
                "foobar-desktop".parse().unwrap(),
                GitSpec {
                    git: git_repo.url.parse().unwrap(),
                    rev: Some(GitReference::Rev(git_repo.commits[0].clone())),
                    subdirectory: Subdirectory::try_from("recipe").unwrap(),
                }
                .into(),
            )]),
            channels: vec![channel_url.clone()],
            build_environment: build_env.clone(),
            channel_config: default_channel_config(),
            ..PixiEnvironmentSpec::default()
        })
        .await
        .unwrap();

    dispatcher
        .install_pixi_environment(InstallPixiEnvironmentSpec {
            name: "test-env".to_owned(),
            records: records.clone(),
            prefix: Prefix::create(&prefix_dir).unwrap(),
            installed: None,
            build_environment: build_env,
            ignore_packages: None,
            force_reinstall: Default::default(),
            channels: vec![channel_url],
            channel_config: default_channel_config(),
            variant_configuration: None,
            variant_files: None,
            enabled_protocols: Default::default(),
        })
        .await
        .unwrap();

    println!(
        "Built the environment successfully: {}",
        prefix_dir.display()
    );

    let event_tree = EventTree::from(events);

    // Redact temp paths and git hashes for stable snapshots
    let output = event_tree.to_string();
    let output = regex::Regex::new(r"file:///[^@]+/multi-output-recipe/")
        .unwrap()
        .replace_all(&output, "file://[LOCAL_GIT_REPO]");
    let output = regex::Regex::new(r"rev=[a-z0-9]+")
        .unwrap()
        .replace_all(&output, "rev=[GIT_REF]");
    let output = regex::Regex::new(r"#[a-f0-9]{40}")
        .unwrap()
        .replace_all(&output, "#[GIT_HASH]");
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

    let dispatcher = CommandDispatcher::builder()
        .with_cache_dirs(default_cache_dirs())
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

    // Busy-wait (briefly) for the "InstantiateToolEnvStarted" event.
    let started = events
        .wait_until_matches(
            |e| matches!(e, Event::InstantiateToolEnvStarted { .. }),
            std::time::Duration::from_secs(2),
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

    // Wait for the background to emit a finished event.
    let finished = events
        .wait_until_matches(
            |e| matches!(e, Event::InstantiateToolEnvFinished { .. }),
            std::time::Duration::from_secs(2),
        )
        .await
        .is_ok();
    assert!(
        finished,
        "instantiate task did not finish promptly after cancellation"
    );

    // Assert: No installation should have started.
    let had_install = events.contains(|e| matches!(e, Event::PixiInstallStarted { .. }));
    assert!(
        !had_install,
        "installation should not have started after cancellation"
    );
}

#[tokio::test]
pub async fn test_cycle() {
    // Setup a reporter that allows us to trace the steps taken by the command
    // dispatcher.
    let (reporter, events) = EventReporter::new();

    // Construct a command dispatcher with:
    // - a root directory located in the `cycle` workspace
    // - the default cache directories but with a temporary workspace cache
    //   directory
    // - the tracing event reporter and a serial executor to trace the flow through
    //   the command dispatcher
    // - the default tool platform and virtual packages
    // - a backend override that uses a passthrough backend to avoid any actual
    //   backend calls
    let (tool_platform, tool_virtual_packages) = tool_platform();
    let root_dir = workspaces_dir().join("cycle");
    let tempdir = tempfile::tempdir().unwrap();
    let dispatcher = CommandDispatcher::builder()
        .with_root_dir(to_abs_dir(root_dir.clone()))
        .with_cache_dirs(default_cache_dirs().with_workspace(to_abs_dir(tempdir.path())))
        .with_reporter(reporter)
        .with_executor(Executor::Serial)
        .with_tool_platform(tool_platform, tool_virtual_packages.clone())
        .with_backend_overrides(BackendOverride::from_memory(
            PassthroughBackend::instantiator(),
        ))
        .finish();

    // Solve an environment with package_a. This should introduce a cycle because
    // package_a depends on package_b, which depends on package_a.
    let error = dispatcher
        .solve_pixi_environment(PixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([(
                "package_a".parse().unwrap(),
                PathSpec {
                    path: "package_a".into(),
                }
                .into(),
            )]),
            build_environment: BuildEnvironment::simple(tool_platform, tool_virtual_packages),
            ..PixiEnvironmentSpec::default()
        })
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
    let records = dispatcher
        .solve_pixi_environment(PixiEnvironmentSpec {
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
            build_environment: build_env.clone(),
            ..PixiEnvironmentSpec::default()
        })
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
                Some(package.name.as_normalized())
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

#[tokio::test]
async fn source_build_cache_status_clear_works() {
    let tmp_dir = tempfile::tempdir().unwrap();

    let dispatcher = CommandDispatcher::builder()
        .with_cache_dirs(CacheDirs::new(to_abs_dir(tmp_dir.path())))
        .finish();

    let host = Platform::current();
    let build_env = BuildEnvironment {
        host_platform: host,
        build_platform: host,
        build_virtual_packages: vec![],
        host_virtual_packages: vec![],
    };

    let pkg = PackageIdentifier {
        name: PackageName::try_from("dummy-pkg").unwrap(),
        version: VersionWithSource::from_str("0.0.0").unwrap(),
        build: "0".to_string(),
        subdir: host.to_string(),
    };

    let spec = SourceBuildCacheStatusSpec {
        package: pkg,
        source: SourceCodeLocation::new(
            PinnedPathSpec {
                path: tmp_dir.path().to_string_lossy().into_owned().into(),
            }
            .into(),
            None,
        ),
        channels: Vec::<ChannelUrl>::new(),
        build_environment: build_env,
        channel_config: default_channel_config(),
        enabled_protocols: Default::default(),
        variants: None,
    };

    let first = dispatcher
        .source_build_cache_status(spec.clone())
        .await
        .expect("query succeeds");

    // Create a weak reference to track that the original Arc is dropped
    // after clearing the cache
    let weak_first = std::sync::Arc::downgrade(&first);

    let second = dispatcher
        .source_build_cache_status(spec.clone())
        .await
        .expect("query succeeds");

    // Cached result should return the same Arc
    assert!(std::sync::Arc::ptr_eq(&first, &second));

    // now drop the cached entries to release the Arc
    // which will unlock the fd locks that we hold on the cache files
    drop(first);
    drop(second);

    // Clear and expect a fresh Arc on next query
    dispatcher.clear_filesystem_caches().await;

    let _third = dispatcher
        .source_build_cache_status(spec)
        .await
        .expect("query succeeds");

    // Check if the original Arc is truly gone
    // and we have a fresh one
    assert!(
        weak_first.upgrade().is_none(),
        "Original Arc should be deallocated after cache clear"
    );
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
            channel_config: default_channel_config(),
            channels: vec![],
            build_environment: BuildEnvironment::simple(tool_platform, tool_virtual_packages),
            variant_configuration: None,
            variant_files: None,
            enabled_protocols: Default::default(),
            preferred_build_source: None,
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
            channel_config: default_channel_config(),
            channels: vec![],
            build_environment: BuildEnvironment::simple(tool_platform, tool_virtual_packages),
            variant_configuration: None,
            variant_files: None,
            enabled_protocols: Default::default(),
            preferred_build_source: None,
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
            channel_config: default_channel_config(),
            channels: vec![],
            build_environment: BuildEnvironment::simple(tool_platform, tool_virtual_packages),
            variant_configuration: Some(variant_config),
            variant_files: None,
            enabled_protocols: Default::default(),
            preferred_build_source: None,
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
    let records = dispatcher
        .solve_pixi_environment(PixiEnvironmentSpec {
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
            build_environment: build_env.clone(),
            ..PixiEnvironmentSpec::default()
        })
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

    // Now we want to rebuild package-b by forcing a rebuild
    let mut spec = InstallPixiEnvironmentSpec::new(records.clone(), prefix);
    spec.force_reinstall = HashSet::from_iter([PackageName::new_unchecked("package-b")]);

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
                Some(package.name.as_normalized())
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
                Some(package.name.as_normalized())
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
                Some(package.name.as_normalized())
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

#[tokio::test]
pub async fn pin_and_checkout_url_reuses_cached_checkout() {
    let tempdir = tempfile::tempdir().unwrap();
    let cache_dirs = CacheDirs::new(to_abs_dir(tempdir.path().join("pixi-cache")));
    let url_cache_root = cache_dirs.url();

    let sha = dummy_sha();
    let checkout_dir = prepare_cached_checkout(url_cache_root.as_std_path(), sha);

    let dispatcher = CommandDispatcher::builder()
        .with_cache_dirs(cache_dirs)
        .with_executor(Executor::Serial)
        .finish();

    // Since we have the same expected hash we expect to return existing archive.
    let spec = UrlSpec {
        url: "https://example.com/archive.tar.gz".parse().unwrap(),
        md5: None,
        sha256: Some(sha),
        subdirectory: Subdirectory::default(),
    };

    let checkout = dispatcher
        .pin_and_checkout_url(spec.clone())
        .await
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

    let dispatcher = CommandDispatcher::builder()
        .with_cache_dirs(cache_dirs)
        .with_executor(Executor::Concurrent)
        .finish();

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
        dispatcher.checkout_url(good_spec),
        dispatcher.checkout_url(bad_spec),
    );

    assert!(good.is_ok());
    assert!(matches!(
        bad,
        Err(CommandDispatcherError::Failed(
            UrlError::Sha256Mismatch { .. }
        ))
    ));
}

#[tokio::test]
pub async fn pin_and_checkout_url_validates_cached_results() {
    let tempdir = tempfile::tempdir().unwrap();
    let cache_dirs = CacheDirs::new(to_abs_dir(tempdir.path().join("pixi-cache")));
    let archive = tempfile::tempdir().unwrap();
    let url = file_url_for_test(&archive, "archive.zip");

    let dispatcher = CommandDispatcher::builder()
        .with_cache_dirs(cache_dirs)
        .with_executor(Executor::Serial)
        .finish();

    let spec = UrlSpec {
        url: url.clone(),
        md5: None,
        sha256: None,
        subdirectory: Subdirectory::default(),
    };

    dispatcher
        .checkout_url(spec.clone())
        .await
        .expect("initial download succeeds");

    let bad_spec = UrlSpec {
        url: url.clone(),
        md5: None,
        sha256: Some(Sha256::digest(b"pixi-url-bad-cache")),
        subdirectory: Subdirectory::default(),
    };

    let err = dispatcher.checkout_url(bad_spec).await.unwrap_err();
    assert!(matches!(
        err,
        CommandDispatcherError::Failed(UrlError::Sha256Mismatch { .. })
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

    let records = dispatcher
        .solve_pixi_environment(PixiEnvironmentSpec {
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
            build_environment: build_env.clone(),
            ..PixiEnvironmentSpec::default()
        })
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
                Some(package.name.as_normalized().to_string())
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

    let records = dispatcher
        .solve_pixi_environment(PixiEnvironmentSpec {
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
            build_environment: build_env.clone(),
            ..PixiEnvironmentSpec::default()
        })
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
                Some(package.name.as_normalized().to_string())
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

    let records = dispatcher
        .solve_pixi_environment(PixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([(
                "package-b".parse().unwrap(),
                PathSpec::new("package-b").into(),
            )]),
            build_environment: build_env.clone(),
            ..PixiEnvironmentSpec::default()
        })
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
                Some(package.name.as_normalized().to_string())
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
    let records = dispatcher
        .solve_pixi_environment(PixiEnvironmentSpec {
            dependencies: DependencyMap::from_iter([(
                "package-b".parse().unwrap(),
                PathSpec::new("package-b").into(),
            )]),
            build_environment: build_env.clone(),
            ..PixiEnvironmentSpec::default()
        })
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
                Some(package.name.as_normalized().to_string())
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
            channel_config: default_channel_config(),
            channels: vec![],
            build_environment: BuildEnvironment::simple(
                tool_platform,
                tool_virtual_packages.clone(),
            ),
            variant_configuration: None,
            variant_files: None,
            enabled_protocols: Default::default(),
            preferred_build_source: None,
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
            channel_config: default_channel_config(),
            channels: vec![],
            build_environment: BuildEnvironment::simple(
                tool_platform,
                tool_virtual_packages.clone(),
            ),
            variant_configuration: None,
            variant_files: None,
            enabled_protocols: Default::default(),
            preferred_build_source: None,
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
