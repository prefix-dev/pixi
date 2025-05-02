//! This crate provides a [`CommandQueue`]. The command queue allows
//! constructing a graph of interdependent operations that can be executed
//! concurrently.
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
//! The API of the [`CommandQueue`] is designed to be used in a way that allows
//! executing a request and awaiting the result. Multiple futures can be created
//! and awaited concurrently.

mod build;
mod cache_dirs;
mod command_queue;
mod command_queue_processor;
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
    use std::path::Path;

    use pixi_spec::PixiSpec;
    use pixi_spec_containers::DependencyMap;
    use rattler_conda_types::{ChannelUrl, Platform};
    use url::Url;

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
        let dispatcher = CommandQueue::builder().executor(Executor::Serial).finish();

        let result = dispatcher
            .solve_pixi_environment(PixiEnvironmentSpec {
                requirements: DependencyMap::from_iter([(
                    "dummy-c".parse().unwrap(),
                    PixiSpec::default(),
                )]),
                channels: vec![local_channel("dummy_channel_1")],
                build_environment: BuildEnvironment::simple_cross(Platform::Linux64).unwrap(),
                ..PixiEnvironmentSpec::default()
            })
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].package_record().name.as_source(), "dummy-c");
    }
}
