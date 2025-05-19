mod event_reporter;

use event_reporter::EventReporter;
use pixi_command_dispatcher::{
    BuildEnvironment, CacheDirs, CommandDispatcher, Executor, PixiEnvironmentSpec,
};
use pixi_spec::{GitReference, GitSpec};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::Platform;
use std::ops::Deref;
use std::str::FromStr;
use url::Url;

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
            requirements: DependencyMap::from_iter([(
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
            build_environment: BuildEnvironment {
                build_platform: Platform::current(),
                ..BuildEnvironment::simple_cross(Platform::Win64).unwrap()
            },
            ..PixiEnvironmentSpec::default()
        })
        .await
        .unwrap();

    insta::assert_json_snapshot!(events.lock().unwrap().deref());
}
