mod event_reporter;
mod event_tree;

use crate::event_tree::EventTree;
use event_reporter::EventReporter;
use pixi_command_dispatcher::{
    BuildEnvironment, CacheDirs, CommandDispatcher, Executor, PixiEnvironmentSpec, SourceBuildSpec,
};
use pixi_config::default_channel_config;
use pixi_spec::{GitReference, GitSpec};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{GenericVirtualPackage, Platform};
use rattler_virtual_packages::{VirtualPackageOverrides, VirtualPackages};
use std::str::FromStr;
use url::Url;

/// Returns a default set of cache directories for the test.
fn default_cache_dirs() -> CacheDirs {
    CacheDirs::new(pixi_config::get_cache_dir().unwrap())
}

/// Returns the tool platform that is appropriate for the current platform.
///
/// Specifically, it normalizes `WinArm64` to `Win64` to increase compatibility.
/// TODO: Once conda-forge supports `WinArm64`, we can remove this normalization.
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
    let dispatcher = CommandDispatcher::builder()
        .with_cache_dirs(default_cache_dirs())
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

    // Find the record for the package we just requested.
    let boost_check_record = records
        .into_iter()
        .filter_map(|r| r.into_source())
        .find(|r| r.package_record.name.as_normalized() == "boost-check")
        .expect("the boost-check package is not part of the solution");

    // Built that package
    let built_source = dispatcher
        .source_build(SourceBuildSpec {
            source: boost_check_record,
            channel_config: default_channel_config(),
            channels: vec![
                Url::from_str("https://prefix.dev/conda-forge")
                    .unwrap()
                    .into(),
            ],
            build_environment: build_env,
            variants: None,
            enabled_protocols: Default::default(),
        })
        .await
        .expect("failed to build the boost-check package");

    println!(
        "Built the package successfully: {}",
        built_source.output_file.display()
    );

    let event_tree = EventTree::new(events.lock().unwrap().iter());
    insta::assert_snapshot!(event_tree.to_string());
}
