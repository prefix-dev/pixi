//! This crate provides a [`CommandQueue`]. The command queue allows
//! constructing a graph of interdependent operations that can be executed
//! concurrently.
//!
//! # Overview
//!
//! For example, solving a pixi environment can entail many different recursive
//! tasks. When source dependencies are part of the environment, we need to
//! check out the source, construct another environment for the build backend,
//! get the metadata from the sourcecode and recursively iterate over any source
//! dependencies returned by the metadata. Pixi also not only does this for one
//! environment, but it needs to do this for multiple environments concurrently.
//!
//! We want to do this as efficiently as possible without recomputing the same
//! information or checking out sources twice. This requires some orchestration.
//! A [`CommandQueue`] is a tool that allows doing this.
//!
//! # Architecture
//!
//! The [`CommandQueue`] is built around a task-based execution model:
//!
//! 1. Each operation is represented as a task implementing the [`TaskSpec`] trait
//! 2. Tasks are submitted to a central queue and processed asynchronously
//! 3. Duplicate tasks are detected and consolidated to avoid redundant work
//! 4. Dependencies between tasks are tracked to ensure proper execution order
//! 5. Results are cached when appropriate to improve performance
//!
//! # Usage
//!
//! The API of the [`CommandQueue`] is designed to be used in a way that allows
//! executing a request and awaiting the result. Multiple futures can be created
//! and awaited concurrently. This enables parallel execution while maintaining
//! a simple API surface.

mod build;
mod cache_dirs;
mod command_queue;
mod command_queue_processor;
mod event_reporter;
mod executor;
mod install_pixi;
mod instantiate_tool_env;
mod limits;
mod reporter;
mod solve_conda;
mod solve_pixi;
mod source_checkout;
mod source_metadata;

pub use build::BuildEnvironment;
pub use cache_dirs::CacheDirs;
pub use command_queue::{
    CommandQueue, CommandQueueError, CommandQueueErrorResultExt, InstantiateBackendError,
    InstantiateBackendSpec,
};
pub use executor::Executor;
pub use reporter::{
    CondaSolveId, CondaSolveReporter, GitCheckoutId, GitCheckoutReporter, PixiInstallReporter,
    PixiSolveId, PixiSolveReporter, Reporter,
};
pub use solve_conda::SolveCondaEnvironmentSpec;
pub use solve_pixi::{PixiEnvironmentSpec, SolvePixiEnvironmentError};
pub use source_checkout::{InvalidPathError, SourceCheckout, SourceCheckoutError};
pub use source_metadata::SourceMetadataSpec;

#[cfg(test)]
mod test {
    use pixi_spec::GitSpec;
    use pixi_spec_containers::DependencyMap;
    use rattler_conda_types::{ChannelUrl, Platform};
    use std::path::Path;
    use std::str::FromStr;
    use url::Url;

    use crate::event_reporter::EventReporter;
    use crate::{BuildEnvironment, CommandQueue, Executor, PixiEnvironmentSpec};

    fn local_channel(name: &str) -> ChannelUrl {
        Url::from_directory_path(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../../tests/data/channels/channels/{name}/")),
        )
        .unwrap()
        .into()
    }

    #[tokio::test]
    pub async fn simple_test() {
        let (reporter, events) = EventReporter::new();
        let dispatcher = CommandQueue::builder()
            .with_reporter(reporter)
            .with_executor(Executor::Serial)
            .finish();

        let result = dispatcher
            .solve_pixi_environment(PixiEnvironmentSpec {
                requirements: DependencyMap::from_iter([(
                    "boost-check".parse().unwrap(),
                    GitSpec {
                        git: "https://github.com/wolfv/pixi-build-examples.git"
                            .parse()
                            .unwrap(),
                        rev: None,
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
                    build_platform: Platform::Win64,
                    ..BuildEnvironment::simple_cross(Platform::Win64).unwrap()
                },
                ..PixiEnvironmentSpec::default()
            })
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].package_record().name.as_source(), "dummy-c");
        insta::assert_debug_snapshot!(&events.lock().unwrap());
    }
}
