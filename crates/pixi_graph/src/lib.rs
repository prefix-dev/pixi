mod conda;
mod dispatcher;
mod reporter;

pub use conda::{CondaEnvironmentSpec, SolveCondaEnvironmentError};
pub use dispatcher::{DispatchError, Dispatcher};

#[cfg(test)]
mod test {
    use std::path::Path;

    use pixi_spec::PixiSpec;
    use pixi_spec_containers::DependencyMap;
    use rattler_conda_types::ChannelUrl;
    use url::Url;

    use crate::{CondaEnvironmentSpec, Dispatcher, SolveCondaEnvironmentError};

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
        let dispatcher = Dispatcher::default();

        let result = dispatcher
            .solve_conda_environment(CondaEnvironmentSpec {
                requirements: DependencyMap::from_iter([(
                    "dummy-c".parse().unwrap(),
                    PixiSpec::default(),
                )]),
                channels: vec![local_channel("dummy_channel_1")],
                ..CondaEnvironmentSpec::default()
            })
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].package_record().name.as_source(), "dummy-c");
    }
}
