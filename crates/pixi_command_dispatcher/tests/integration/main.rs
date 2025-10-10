mod event_reporter;
mod event_tree;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    // ptr,
    str::FromStr,
};

use event_reporter::EventReporter;
use itertools::Itertools;
use pixi_build_backend_passthrough::PassthroughBackend;
use pixi_build_frontend::{BackendOverride, InMemoryOverriddenBackends};
use pixi_command_dispatcher::{
    BuildEnvironment, CacheDirs, CommandDispatcher, Executor, GetOutputDependenciesSpec,
    InstallPixiEnvironmentSpec, InstantiateToolEnvironmentSpec, PackageIdentifier,
    PixiEnvironmentSpec, SourceBuildCacheStatusSpec,
};
use pixi_config::default_channel_config;
use pixi_record::PinnedPathSpec;
use pixi_spec::{GitReference, GitSpec, PathSpec, PixiSpec};
use pixi_spec_containers::DependencyMap;
use pixi_test_utils::format_diagnostic;
use rattler_conda_types::{
    ChannelUrl, GenericVirtualPackage, PackageName, Platform, VersionSpec, VersionWithSource,
    prefix::Prefix,
};
use rattler_virtual_packages::{VirtualPackageOverrides, VirtualPackages};
use url::Url;

use crate::{event_reporter::Event, event_tree::EventTree};

/// Returns a default set of cache directories for the test.
fn default_cache_dirs() -> CacheDirs {
    CacheDirs::new(pixi_config::get_cache_dir().unwrap())
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

/// Returns the default build environment to use for tests.
fn default_build_environment() -> BuildEnvironment {
    let (tool_platform, tool_virtual_packages) = tool_platform();
    BuildEnvironment::simple(tool_platform, tool_virtual_packages)
}

#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
pub async fn simple_test() {
    let (reporter, events) = EventReporter::new();
    let (tool_platform, tool_virtual_packages) = tool_platform();
    let tempdir = tempfile::tempdir().unwrap();
    let prefix_dir = tempdir.path().join("prefix");
    let dispatcher = CommandDispatcher::builder()
        .with_cache_dirs(default_cache_dirs().with_workspace(tempdir.path().to_path_buf()))
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
                    git: "https://github.com/wolfv/pixi-build-examples.git"
                        .parse()
                        .unwrap(),
                    rev: Some(GitReference::Rev(
                        "8d230eda9b4cdaaefd24aad87fd923d4b7c3c78a".to_owned(),
                    )),
                    subdirectory: Some(String::from("multi-output/recipe")),
                }
                .into(),
            )]),
            channels: vec![
                Url::from_str("https://prefix.dev/conda-forge")
                    .unwrap()
                    .into(),
            ],
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
            channels: vec![
                Url::from_str("https://prefix.dev/conda-forge")
                    .unwrap()
                    .into(),
            ],
            channel_config: default_channel_config(),
            variants: None,
            enabled_protocols: Default::default(),
        })
        .await
        .unwrap();

    println!(
        "Built the environment successfully: {}",
        prefix_dir.display()
    );

    let event_tree = EventTree::from(events);
    insta::assert_snapshot!(event_tree.to_string());
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

    // Exactly one result should be a failure and the other should be cancelled.
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
        cancelled_count, 1,
        "expected exactly one request to be cancelled"
    );
    assert_eq!(failed_count, 1, "expected exactly one request to fail");
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
    let mut write_guard = pixi_utils::AsyncPrefixGuard::new(&prefix_dir)
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
        .with_root_dir(root_dir.clone())
        .with_cache_dirs(default_cache_dirs().with_workspace(tempdir.path().to_path_buf()))
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
    // Construct a command dispatcher with:
    // - a root directory located in the `cycle` workspace
    // - the default cache directories but with a temporary workspace cache
    //   directory
    // - the default tool platform and virtual packages
    // - a backend override that uses a passthrough backend to avoid any actual
    //   backend calls
    let root_dir = workspaces_dir().join("host-dependency");
    let tempdir = tempfile::tempdir().unwrap();
    let (tool_platform, tool_virtual_packages) = tool_platform();
    let build_env = BuildEnvironment::simple(tool_platform, tool_virtual_packages.clone());
    let build_command_dispatcher = || {
        CommandDispatcher::builder()
            .with_root_dir(root_dir.clone())
            .with_cache_dirs(default_cache_dirs().with_workspace(tempdir.path().to_path_buf()))
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
        vec!["package-a", "package-b"],
        "Expected only package-a and package-b to be rebuilt"
    );
}

#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
pub async fn instantiate_backend_with_from_source() {
    let root_dir = workspaces_dir().join("source-backends");

    let dispatcher = CommandDispatcher::builder()
        .with_root_dir(root_dir.clone())
        .with_cache_dirs(default_cache_dirs())
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
        .with_cache_dirs(CacheDirs::new(tmp_dir.path().to_path_buf()))
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
        source: PinnedPathSpec {
            path: tmp_dir.path().to_string_lossy().into_owned().into(),
        }
        .into(),
        channels: Vec::<ChannelUrl>::new(),
        build_environment: build_env,
        channel_config: default_channel_config(),
        enabled_protocols: Default::default(),
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

/// Tests that `get_output_dependencies` correctly retrieves build, host, and run
/// dependencies for a specific output from a source package using the in-memory
/// backend.
#[tokio::test]
pub async fn test_get_output_dependencies() {
    // Setup: Create a dispatcher with the in-memory backend
    let root_dir = workspaces_dir().join("output-dependencies");
    let tempdir = tempfile::tempdir().unwrap();
    let (tool_platform, tool_virtual_packages) = tool_platform();

    let dispatcher = CommandDispatcher::builder()
        .with_root_dir(root_dir.clone())
        .with_cache_dirs(default_cache_dirs().with_workspace(tempdir.path().to_path_buf()))
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

    // Create the spec for getting output dependencies
    let spec = GetOutputDependenciesSpec {
        source: pinned_source,
        output_name: PackageName::new_unchecked("test-package"),
        channel_config: default_channel_config(),
        channels: vec![],
        build_environment: BuildEnvironment::simple(tool_platform, tool_virtual_packages),
        variants: None,
        enabled_protocols: Default::default(),
    };

    // Act: Get the output dependencies
    let result = dispatcher
        .get_output_dependencies(spec)
        .await
        .map_err(|e| format_diagnostic(&e))
        .expect("get_output_dependencies should succeed");

    // Assert: Verify the dependencies are returned correctly
    assert!(
        result.build_dependencies.is_some(),
        "Build dependencies should be present"
    );
    assert!(
        result.host_dependencies.is_some(),
        "Host dependencies should be present"
    );

    let build_deps = result.build_dependencies.unwrap();
    let host_deps = result.host_dependencies.unwrap();
    let run_deps = &result.run_dependencies;

    // Verify build dependencies (cmake, make)
    let build_dep_names: Vec<_> = build_deps
        .names()
        .map(|name| name.as_normalized())
        .sorted()
        .collect();
    assert_eq!(
        build_dep_names,
        vec!["cmake", "make"],
        "Build dependencies should include cmake and make"
    );

    // Verify host dependencies (zlib, openssl)
    let host_dep_names: Vec<_> = host_deps
        .names()
        .map(|name| name.as_normalized())
        .sorted()
        .collect();
    assert_eq!(
        host_dep_names,
        vec!["openssl", "zlib"],
        "Host dependencies should include zlib and openssl"
    );

    // Verify run dependencies (python, numpy)
    let run_dep_names: Vec<_> = run_deps
        .names()
        .map(|name| name.as_normalized())
        .sorted()
        .collect();
    assert_eq!(
        run_dep_names,
        vec!["numpy", "python"],
        "Run dependencies should include python and numpy"
    );

    // Verify constraints are empty (our test package doesn't have any)
    assert!(
        result
            .build_constraints
            .as_ref()
            .map_or(true, |c| c.is_empty()),
        "Build constraints should be empty"
    );
    assert!(
        result
            .host_constraints
            .as_ref()
            .map_or(true, |c| c.is_empty()),
        "Host constraints should be empty"
    );
    assert!(
        result.run_constraints.is_empty(),
        "Run constraints should be empty"
    );
}

/// Tests that `get_output_dependencies` returns an appropriate error when the
/// specified output is not found in the source package.
#[tokio::test]
pub async fn test_get_output_dependencies_output_not_found() {
    // Setup: Create a dispatcher with the in-memory backend
    let root_dir = workspaces_dir().join("output-dependencies");
    let tempdir = tempfile::tempdir().unwrap();
    let (tool_platform, tool_virtual_packages) = tool_platform();

    let dispatcher = CommandDispatcher::builder()
        .with_root_dir(root_dir.clone())
        .with_cache_dirs(default_cache_dirs().with_workspace(tempdir.path().to_path_buf()))
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

    // Create the spec with a non-existent output name
    let spec = GetOutputDependenciesSpec {
        source: pinned_source,
        output_name: PackageName::new_unchecked("non-existent-output"),
        channel_config: default_channel_config(),
        channels: vec![],
        build_environment: BuildEnvironment::simple(tool_platform, tool_virtual_packages),
        variants: None,
        enabled_protocols: Default::default(),
    };

    // Act: Try to get the output dependencies
    let error = dispatcher
        .get_output_dependencies(spec)
        .await
        .expect_err("Expected OutputNotFound error");

    // Assert: Verify we got the expected error type
    use pixi_command_dispatcher::{CommandDispatcherError, GetOutputDependenciesError};
    match error {
        CommandDispatcherError::Failed(GetOutputDependenciesError::OutputNotFound {
            output_name,
            available_outputs,
        }) => {
            assert_eq!(
                output_name.as_source(),
                "non-existent-output",
                "Error should contain the requested output name"
            );
            assert_eq!(
                available_outputs.len(),
                1,
                "Should have one available output"
            );
            assert_eq!(
                available_outputs[0].as_source(),
                "test-package",
                "Available outputs should include test-package"
            );
        }
        other => panic!(
            "Expected OutputNotFound error, got: {}",
            format_diagnostic(&other)
        ),
    }
}

/// Tests that `expand_dev_sources` correctly extracts dependencies from dev
/// sources and allows them to be merged into a PixiEnvironmentSpec.
#[tokio::test]
pub async fn test_expand_dev_sources() {
    use pixi_command_dispatcher::{DependencyOnlySource, ExpandDevSourcesSpec};
    use pixi_spec::{PathSourceSpec, SourceSpec};

    // Setup: Create a dispatcher with the in-memory backend
    let root_dir = workspaces_dir().join("output-dependencies");
    let tempdir = tempfile::tempdir().unwrap();
    let (tool_platform, tool_virtual_packages) = tool_platform();

    let dispatcher = CommandDispatcher::builder()
        .with_root_dir(root_dir.clone())
        .with_cache_dirs(default_cache_dirs().with_workspace(tempdir.path().to_path_buf()))
        .with_executor(Executor::Serial)
        .with_tool_platform(tool_platform, tool_virtual_packages.clone())
        .with_backend_overrides(BackendOverride::from_memory(
            PassthroughBackend::instantiator(),
        ))
        .finish();

    // Create dev sources for test-package and package-a
    // package-a depends on test-package (also a dev source, should be filtered)
    // and on package-b (not a dev source, should be included)
    let dev_sources = vec![
        DependencyOnlySource {
            source: SourceSpec::from(PathSourceSpec {
                path: "test-package".into(),
            }),
            output_name: PackageName::new_unchecked("test-package"),
        },
        DependencyOnlySource {
            source: SourceSpec::from(PathSourceSpec {
                path: "package-a".into(),
            }),
            output_name: PackageName::new_unchecked("package-a"),
        },
    ];

    // Create the spec for expanding dev sources
    let spec = ExpandDevSourcesSpec {
        dev_sources,
        channel_config: default_channel_config(),
        channels: vec![],
        build_environment: BuildEnvironment::simple(tool_platform, tool_virtual_packages.clone()),
        variants: None,
        enabled_protocols: Default::default(),
    };

    // Act: Expand the dev sources
    let expanded = dispatcher
        .expand_dev_sources(spec)
        .await
        .map_err(|e| format_diagnostic(&e))
        .expect("expand_dev_sources should succeed");

    // Print the expanded dependencies in JSON format for easy inspection
    println!("\n=== Expanded Dependencies ===");
    let json_string = serde_json::to_string_pretty(&expanded.dependencies)
        .expect("Failed to serialize dependencies to JSON");
    println!("{}", json_string);

    if !expanded.constraints.is_empty() {
        println!("\n=== Expanded Constraints ===");
        let constraints_json = serde_json::to_string_pretty(&expanded.constraints)
            .expect("Failed to serialize constraints to JSON");
        println!("{}", constraints_json);
    }
    println!("=============================\n");

    // Assert: Verify all dependencies were extracted
    let all_dep_names: Vec<_> = expanded
        .dependencies
        .names()
        .map(|name| name.as_normalized())
        .sorted()
        .collect();

    // Expected dependencies:
    // - From test-package: cmake, make (build), openssl, zlib (host), numpy, python (run)
    // - From package-a: gcc (build), requests (run), package-b (run, which is a source)
    // - test-package is NOT included even though package-a depends on it (filtered because it's a dev source)
    // - package-b IS included (not a dev source)
    assert_eq!(
        all_dep_names,
        vec![
            "cmake",
            "gcc",
            "make",
            "numpy",
            "openssl",
            "package-b",
            "python",
            "requests",
            "zlib"
        ],
        "All dependencies should be extracted, with dev sources filtered out"
    );

    // Verify that test-package is NOT in the dependencies (it's filtered because it's a dev source)
    assert!(
        !expanded.dependencies.contains_key("test-package"),
        "test-package should be filtered out because it's a dev source"
    );

    // Verify that package-a is NOT in the dependencies (it's filtered because it's a dev source)
    assert!(
        !expanded.dependencies.contains_key("package-a"),
        "package-a should be filtered out because it's a dev source"
    );

    // Verify that package-b IS in the dependencies (it's not a dev source)
    assert!(
        expanded.dependencies.contains_key("package-b"),
        "package-b should be included because it's not a dev source"
    );

    // Assert: Verify constraints are empty (test packages have no constraints)
    assert!(
        expanded.constraints.is_empty(),
        "Test packages have no constraints"
    );
}
