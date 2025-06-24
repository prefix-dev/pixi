use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

use futures::StreamExt;
use miette::Diagnostic;
use pixi_build_discovery::EnabledProtocols;
use pixi_record::{PinnedSourceSpec, SourceRecord};
use pixi_spec::{SourceAnchor, SourceSpec};
use rattler_conda_types::{ChannelConfig, ChannelUrl, MatchSpec, ParseStrictness};
use thiserror::Error;

use crate::{
    BuildBackendMetadataSpec, BuildEnvironment, CommandDispatcher, CommandDispatcherError,
    CommandDispatcherErrorResultExt, SourceCheckoutError, SourceMetadataSpec,
    executor::ExecutorFutures,
    source_metadata::{SourceMetadata, SourceMetadataError},
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
    #[error("failed to extract metadata for package '{name}'")]
    SourceMetadataError {
        name: String,
        #[source]
        #[diagnostic_source]
        error: SourceMetadataError,
    },
    #[error("the package '{name}' is not provided by the project located at '{}'", &.pinned_source)]
    PackageMetadataNotFound {
        name: String,
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
        let mut specs = specs;
        let mut result = CollectedSourceMetadata::default();
        let mut already_encountered_specs = HashSet::new();

        loop {
            // Create futures for all encountered specs.
            for (name, spec) in specs.drain(..) {
                if already_encountered_specs.insert(spec.clone()) {
                    source_futures.push(self.collect_source_metadata(name, spec));
                }
            }

            // Wait for the next future to finish.
            let Some(source_metadata) = source_futures.next().await else {
                // No more pending futures, we are done.
                return Ok(result);
            };

            // Handle any potential error
            let source_metadata = source_metadata?;

            // Process transitive dependencies
            for record in &source_metadata.records {
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
                            specs.push((name, anchor.resolve(source_spec)));
                        } else {
                            // We encountered a transitive dependency that is not a source
                            result.transitive_dependencies.push(spec);
                        }
                    } else {
                        // TODO: Should we handle this error?
                    }
                }
            }

            result.source_repodata.push(source_metadata);
        }
    }

    async fn collect_source_metadata(
        &self,
        name: rattler_conda_types::PackageName,
        spec: SourceSpec,
    ) -> Result<Arc<SourceMetadata>, CommandDispatcherError<CollectSourceMetadataError>> {
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
        let source_metadata = self
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
            .map_err_with(|err| CollectSourceMetadataError::SourceMetadataError {
                name: name.as_source().to_string(),
                error: err,
            })?;

        // Make sure that a package with the name defined in spec is available from the
        // backend.
        if source_metadata.records.is_empty() {
            return Err(CommandDispatcherError::Failed(
                CollectSourceMetadataError::PackageMetadataNotFound {
                    name: name.as_source().to_string(),
                    pinned_source: Box::new(source_metadata.source.clone()),
                    help: Self::create_metadata_not_found_help(
                        name,
                        source_metadata.records.clone(),
                    ),
                },
            ));
        }

        Ok(source_metadata)
    }

    /// Create a help message for the user when the requested package is not
    /// found in the metadata returned by a backend.
    fn create_metadata_not_found_help(
        name: rattler_conda_types::PackageName,
        records: Vec<SourceRecord>,
    ) -> String {
        records
            .into_iter()
            .map(|record| {
                (
                    strsim::jaro(
                        record.package_record.name.as_normalized(),
                        name.as_normalized(),
                    ),
                    record,
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
                |record| {
                    format!(
                        "The build backend does provide other packages, did you mean '{}'?",
                        record.package_record.name.as_normalized()
                    )
                },
            )
    }
}
