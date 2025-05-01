mod command_queue;
mod conda;
mod pixi;
mod reporter;
mod source_checkout;
mod source_metadata;

pub use command_queue::{CommandQueue, CommandQueueError};
pub use conda::{CondaEnvironmentSpec, SolveCondaEnvironmentError};
pub use pixi::{PixiEnvironmentSpec, SolvePixiEnvironmentError};
pub use reporter::{CondaSolveReporter, Reporter, SolveId};
pub use source_checkout::{InvalidPathError, SourceCheckout, SourceCheckoutError};
pub use source_metadata::SourceMetadataSpec;

#[cfg(test)]
mod test {
    use std::path::Path;

    use pixi_spec::PixiSpec;
    use pixi_spec_containers::DependencyMap;
    use rattler_conda_types::ChannelUrl;
    use url::Url;

    use crate::{CommandQueue, PixiEnvironmentSpec};

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
        let dispatcher = CommandQueue::default();

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
