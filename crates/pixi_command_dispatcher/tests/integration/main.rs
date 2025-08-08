mod event_reporter;
mod event_tree;

use std::{
    collections::HashMap,
    ops::DerefMut,
    path::{Path, PathBuf},
    str::FromStr,
};

use event_reporter::EventReporter;
use itertools::Itertools;
use pixi_build_frontend::{
    BackendOverride, InMemoryOverriddenBackends, in_memory::PassthroughBackend,
};
use pixi_command_dispatcher::{
    BuildEnvironment, CacheDirs, CommandDispatcher, Executor, InstallPixiEnvironmentSpec,
    InstantiateToolEnvironmentSpec, PixiEnvironmentSpec,
};
use pixi_config::default_channel_config;
use pixi_spec::{GitReference, GitSpec, PathSpec, PixiSpec};
use pixi_spec_containers::DependencyMap;
use pixi_test_utils::format_diagnostic;
use rattler_conda_types::{
    GenericVirtualPackage, PackageName, Platform, VersionSpec, prefix::Prefix,
};
use rattler_virtual_packages::{VirtualPackageOverrides, VirtualPackages};
use url::Url;

use crate::event_tree::EventTree;

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

    let event_tree = EventTree::new(events.lock().unwrap().iter());
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
    let event_tree = EventTree::new(events.lock().unwrap().iter());
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
    let first_events = std::mem::take(first_events.lock().unwrap().deref_mut());
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
    let second_events = std::mem::take(second_events.lock().unwrap().deref_mut());
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
