mod event_reporter;
mod event_tree;

use std::{path::Path, str::FromStr};

use event_reporter::EventReporter;
use pixi_command_dispatcher::{
    BuildEnvironment, CacheDirs, CommandDispatcher, Executor, InstallPixiEnvironmentSpec,
    InstantiateToolEnvironmentSpec, PixiEnvironmentSpec,
};
use pixi_config::default_channel_config;
use pixi_spec::{GitReference, GitSpec, PixiSpec};
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

    let build_env = BuildEnvironment {
        host_platform: tool_platform,
        host_virtual_packages: tool_virtual_packages.clone(),
        build_platform: tool_platform,
        build_virtual_packages: tool_virtual_packages.clone(),
    };

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
            target_platform: tool_platform,
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
