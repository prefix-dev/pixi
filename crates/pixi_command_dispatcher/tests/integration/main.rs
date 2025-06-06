mod event_reporter;
mod event_tree;

use std::str::FromStr;

use event_reporter::EventReporter;
use pixi_command_dispatcher::{
    BuildEnvironment, CacheDirs, CommandDispatcher, Executor, InstallPixiEnvironmentSpec,
    PixiEnvironmentSpec, SourceBuildSpec,
};
use pixi_config::default_channel_config;
use pixi_spec::{GitReference, GitSpec};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{GenericVirtualPackage, Platform, prefix::Prefix};
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
                "boost-check".parse().unwrap(),
                GitSpec {
                    git: "https://github.com/wolfv/pixi-build-examples.git"
                        .parse()
                        .unwrap(),
                    rev: Some(GitReference::Rev(
                        "a4c27e86a4a5395759486552abb3df8a47d50172".to_owned(),
                    )),
                    subdirectory: Some(String::from("boost-check")),
                }
                .into(),
            )]),
            channels: vec![
                Url::from_str("https://prefix.dev/conda-forge")
                    .unwrap()
                    .into(),
            ],
            build_environment: build_env.clone(),
            ..PixiEnvironmentSpec::default()
        })
        .await
        .unwrap();

    dispatcher
        .install_pixi_environment(InstallPixiEnvironmentSpec {
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
