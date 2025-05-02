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
mod conda;
mod executor;
mod limits;
mod pixi;
mod reporter;
mod source_checkout;
mod source_metadata;

pub use build::BuildEnvironment;
pub use command_queue::{CommandQueue, CommandQueueError, CommandQueueErrorResultExt};
pub use conda::SolveCondaEnvironmentSpec;
pub use executor::Executor;
pub use pixi::{PixiEnvironmentSpec, SolvePixiEnvironmentError};
pub use reporter::{
    CondaSolveId, CondaSolveReporter, GitCheckoutId, GitCheckoutReporter, PixiSolveId,
    PixiSolveReporter, Reporter,
};
pub use source_checkout::{InvalidPathError, SourceCheckout, SourceCheckoutError};
pub use source_metadata::SourceMetadataSpec;

#[cfg(test)]
mod test {
    use std::path::Path;

    use pixi_spec::PixiSpec;
    use pixi_spec_containers::DependencyMap;
    use rattler_conda_types::ChannelUrl;
    use url::Url;

    use crate::{CommandQueue, Executor, PixiEnvironmentSpec};

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
        let dispatcher = CommandQueue::builder()
            .executor(Executor::Serial)
            .finish();

        let result = dispatcher
            .solve_pixi_environment(PixiEnvironmentSpec {
                requirements: DependencyMap::from_iter([(
                    "dummy-c".parse().unwrap(),
                    PixiSpec::default(),
                )]),
                channels: vec![local_channel("dummy_channel_1")],
                ..PixiEnvironmentSpec::default()
            })
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].package_record().name.as_source(), "dummy-c");
    }
}
