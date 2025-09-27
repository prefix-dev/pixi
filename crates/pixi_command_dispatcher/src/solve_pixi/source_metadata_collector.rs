use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

use futures::{FutureExt, StreamExt};
use miette::Diagnostic;
use pixi_build_discovery::EnabledProtocols;
use pixi_record::PinnedSourceSpec;
use pixi_spec::{SourceAnchor, SourceSpec};
use rattler_conda_types::{ChannelConfig, ChannelUrl, MatchSpec, ParseStrictness};
use thiserror::Error;

use crate::{
    BuildBackendMetadataSpec, BuildEnvironment, CommandDispatcher, CommandDispatcherError,
    SourceCheckoutError, SourceMetadataSpec,
    executor::ExecutorFutures,
    source_metadata::{CycleEnvironment, SourceMetadata, SourceMetadataError},
};

/// An object that is responsible for recursively collecting metadata of source
/// dependencies.
pub struct SourceMetadataCollector {
    command_queue: CommandDispatcher,
    channel_config: ChannelConfig,
    channels: Vec<ChannelUrl>,
    build_environment: BuildEnvironment,
    enabled_protocols: EnabledProtocols,
    variants: Option<BTreeMap<String, Vec<String>>>,
}

#[derive(Default)]
pub struct CollectedSourceMetadata {
    /// Information about all queried source packages. This can be used as
    /// repodata.
    pub source_repodata: Vec<Arc<SourceMetadata>>,

    /// A list of transitive dependencies of all collected source records.
    pub transitive_dependencies: Vec<MatchSpec>,
}

/// An error that can occur while collecting source metadata.
#[derive(Debug, Error, Diagnostic)]
pub enum CollectSourceMetadataError {
    #[error("failed to extract metadata for package '{}'", .name.as_source())]
    SourceMetadataError {
        name: rattler_conda_types::PackageName,
        #[source]
        #[diagnostic_source]
        error: SourceMetadataError,
    },
    #[error("the package '{}' is not provided by the project located at '{}'", .name.as_source(), &.pinned_source)]
    PackageMetadataNotFound {
        name: rattler_conda_types::PackageName,
        pinned_source: Box<PinnedSourceSpec>,
        #[help]
        help: String,
    },
    #[error("failed to checkout source for package '{name}'")]
    SourceCheckoutError {
        name: String,
        #[source]
        #[diagnostic_source]
        error: CommandDispatcherError<SourceCheckoutError>,
    },
}

impl SourceMetadataCollector {
    pub fn new(
        command_queue: CommandDispatcher,
        channel_urls: Vec<ChannelUrl>,
        channel_config: ChannelConfig,
        build_environment: BuildEnvironment,
        variants: Option<BTreeMap<String, Vec<String>>>,
        enabled_protocols: EnabledProtocols,
    ) -> Self {
        Self {
            command_queue,
            channels: channel_urls,
            build_environment,
            enabled_protocols,
            channel_config,
            variants,
        }
    }

    pub async fn collect(
        self,
        specs: Vec<(rattler_conda_types::PackageName, SourceSpec)>,
    ) -> Result<CollectedSourceMetadata, CommandDispatcherError<CollectSourceMetadataError>> {
        let mut source_futures = ExecutorFutures::new(self.command_queue.executor());
        let mut specs = specs
            .into_iter()
            .map(|(name, spec)| (name, spec, Vec::new()))
            .collect::<Vec<_>>();
        let mut result = CollectedSourceMetadata::default();
        let mut already_encountered_specs = HashSet::new();

        loop {
            // Create futures for all encountered specs.
            for (name, spec, chain) in specs.drain(..) {
                if already_encountered_specs.insert(spec.clone()) {
                    source_futures.push(
                        self.collect_source_metadata(name, spec, chain)
                            .boxed_local(),
                    );
                }
            }

            // Wait for the next future to finish.
            let Some(source_metadata) = source_futures.next().await else {
                // No more pending futures, we are done.
                return Ok(result);
            };

            // Handle any potential error
            let (source_metadata, mut chain) = source_metadata?;

            // Process transitive dependencies
            for record in &source_metadata.records {
                chain.push(record.package_record.name.clone());
                let anchor = SourceAnchor::from(SourceSpec::from(record.source.clone()));
                for depend in &record.package_record.depends {
                    if let Ok(spec) = MatchSpec::from_str(depend, ParseStrictness::Lenient) {
                        if let Some((name, source_spec)) = spec.name.as_ref().and_then(|name| {
                            record
                                .sources
                                .get(name.as_normalized())
                                .map(|source_spec| (name.clone(), source_spec.clone()))
                        }) {
                            // We encountered a transitive source dependency.
                            specs.push((name, anchor.resolve(source_spec), chain.clone()));
                        } else {
                            // We encountered a transitive dependency that is not a source
                            result.transitive_dependencies.push(spec);
                        }
                    } else {
                        // TODO: Should we handle this error?
                    }
                }
                chain.pop();
            }

            result.source_repodata.push(source_metadata);
        }
    }

    async fn collect_source_metadata(
        &self,
        name: rattler_conda_types::PackageName,
        spec: SourceSpec,
        chain: Vec<rattler_conda_types::PackageName>,
    ) -> Result<
        (Arc<SourceMetadata>, Vec<rattler_conda_types::PackageName>),
        CommandDispatcherError<CollectSourceMetadataError>,
    > {
        tracing::trace!("Collecting source metadata for {name:#?}");

        // Get the source for the particular package.
        let source = self
            .command_queue
            .pin_and_checkout(spec)
            .await
            .map_err(|err| CollectSourceMetadataError::SourceCheckoutError {
                name: name.as_source().to_string(),
                error: err,
            })
            .map_err(CommandDispatcherError::Failed)?;

        // Extract information for the particular source spec.
        let source_metadata = match self
            .command_queue
            .source_metadata(SourceMetadataSpec {
                package: name.clone(),
                backend_metadata: BuildBackendMetadataSpec {
                    source: source.pinned,
                    channel_config: self.channel_config.clone(),
                    channels: self.channels.clone(),
                    build_environment: self.build_environment.clone(),
                    variants: self.variants.clone(),
                    enabled_protocols: self.enabled_protocols.clone(),
                },
            })
            .await
        {
            Err(CommandDispatcherError::Cancelled) => {
                return Err(CommandDispatcherError::Cancelled);
            }
            Err(CommandDispatcherError::Failed(SourceMetadataError::Cycle(mut cycle))) => {
                // Push the packages that led up to this cycle onto the cycle stack.
                cycle
                    .stack
                    .extend(chain.into_iter().map(|pkg| (pkg, CycleEnvironment::Run)));
                return Err(CommandDispatcherError::Failed(
                    CollectSourceMetadataError::SourceMetadataError {
                        name,
                        error: SourceMetadataError::Cycle(cycle),
                    },
                ));
            }
            Err(CommandDispatcherError::Failed(error)) => {
                return Err(CommandDispatcherError::Failed(
                    CollectSourceMetadataError::SourceMetadataError { name, error },
                ));
            }
            Ok(metadata) => metadata,
        };

        // Make sure that a package with the name defined in spec is available from the
        // backend.
        if source_metadata.records.is_empty() {
            return Err(CommandDispatcherError::Failed(
                CollectSourceMetadataError::PackageMetadataNotFound {
                    help: Self::create_metadata_not_found_help(
                        &name,
                        source_metadata.skipped_packages.clone(),
                    ),
                    name,
                    pinned_source: Box::new(source_metadata.source.clone()),
                },
            ));
        }

        Ok((source_metadata, chain))
    }

    /// Create a help message for the user when the requested package is not
    /// found in the metadata returned by a backend.
    fn create_metadata_not_found_help(
        name: &rattler_conda_types::PackageName,
        skipped_packages: Vec<rattler_conda_types::PackageName>,
    ) -> String {
        skipped_packages
            .into_iter()
            .map(|skipped_name| {
                (
                    strsim::jaro(skipped_name.as_normalized(), name.as_normalized()),
                    skipped_name,
                )
            })
            .max_by(|(score_a, _), (score_b, _)| {
                score_a
                    .partial_cmp(score_b)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(_, record)| record)
            .map_or_else(
                || String::from("No packages are provided by the build-backend"),
                |skipped_name| {
                    format!(
                        "The build backend does provide other packages, did you mean '{}'?",
                        skipped_name.as_normalized(),
                    )
                },
            )
    }
}
