mod event_reporter;
mod event_tree;

use std::{path::PathBuf, str::FromStr};

use event_reporter::EventReporter;
use pixi_build_discovery::EnabledProtocols;
use pixi_command_dispatcher::{
    BuildEnvironment, CacheDirs, CommandDispatcher, Executor, InstantiateToolEnvironmentSpec,
    PixiEnvironmentSpec,
};
use pixi_spec::{GitReference, GitSpec, PixiSpec};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{ChannelConfig, ChannelUrl, PackageName, VersionSpec};
use tempfile::TempDir;
use url::Url;

use crate::{
    command_dispatcher::event_tree::EventTree,
    common::package_database::{Package, PackageDatabase},
};

/// Returns a default set of cache directories for the test.
fn default_cache_dirs() -> CacheDirs {
    CacheDirs::new(pixi_config::get_cache_dir().unwrap())
}

#[tokio::test]
pub async fn simple_test() {
    let (reporter, events) = EventReporter::new();
    let dispatcher = CommandDispatcher::builder()
        .with_cache_dirs(default_cache_dirs())
        .with_reporter(reporter)
        .with_executor(Executor::Serial)
        .finish();

    let _result = dispatcher
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
            ..PixiEnvironmentSpec::default()
        })
        .await
        .unwrap();

    let event_tree = EventTree::new(events.lock().unwrap().iter());
    insta::assert_snapshot!(event_tree.to_string());
}
