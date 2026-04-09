use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use futures::{FutureExt, StreamExt};
use miette::Diagnostic;
use pixi_build_discovery::EnabledProtocols;
use pixi_record::{PinnedSourceSpec, SourceRecordReuseKey, VariantValue};
use pixi_spec::{ResolvedExcludeNewer, SourceAnchor, SourceLocationSpec, SourceSpec};
use rattler_conda_types::{
    ChannelConfig, ChannelUrl, MatchSpec, PackageNameMatcher, ParseStrictness,
};
use thiserror::Error;

use crate::{
    BuildBackendMetadataSpec, BuildEnvironment, CommandDispatcher, CommandDispatcherError,
    PackageNotProvidedError, SourceCheckoutError, SourceMetadataSpec, SourceRecordError,
    executor::CancellationAwareFutures,
    source_metadata::{CycleEnvironment, SourceMetadata, SourceMetadataError},
};

/// An object that is responsible for recursively collecting metadata of source
/// dependencies.
pub struct SourceMetadataCollector {
    command_queue: CommandDispatcher,
    channel_config: ChannelConfig,
    channels: Vec<ChannelUrl>,
    build_environment: BuildEnvironment,
    exclude_newer: Option<ResolvedExcludeNewer>,
    enabled_protocols: EnabledProtocols,
    variant_configuration: Option<BTreeMap<String, Vec<VariantValue>>>,
    variant_files: Option<Vec<PathBuf>>,
    preferred_build_sources: BTreeMap<rattler_conda_types::PackageName, PinnedSourceSpec>,
    source_timestamp_hints: HashMap<SourceRecordReuseKey, pixi_spec::SourceTimestamps>,
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
#[derive(Debug, Clone, Error, Diagnostic)]
pub enum CollectSourceMetadataError {
    #[error("failed to extract metadata for package '{}'", .name.as_source())]
    SourceMetadataError {
        name: rattler_conda_types::PackageName,
        #[source]
        #[diagnostic_source]
        error: SourceMetadataError,
    },
    #[error(transparent)]
    #[diagnostic(transparent)]
    PackageNotProvided(#[from] PackageNotProvidedError),
    #[error("failed to checkout source for package '{name}'")]
    SourceCheckoutError {
        name: String,
        #[source]
        #[diagnostic_source]
        error: CommandDispatcherError<SourceCheckoutError>,
    },
}

impl SourceMetadataCollector {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        command_queue: CommandDispatcher,
        channel_urls: Vec<ChannelUrl>,
        channel_config: ChannelConfig,
        build_environment: BuildEnvironment,
        exclude_newer: Option<ResolvedExcludeNewer>,
        variant_configuration: Option<BTreeMap<String, Vec<VariantValue>>>,
        variant_files: Option<Vec<PathBuf>>,
        enabled_protocols: EnabledProtocols,
        preferred_build_sources: BTreeMap<rattler_conda_types::PackageName, PinnedSourceSpec>,
        source_timestamp_hints: HashMap<SourceRecordReuseKey, pixi_spec::SourceTimestamps>,
    ) -> Self {
        Self {
            command_queue,
            channels: channel_urls,
            build_environment,
            exclude_newer,
            enabled_protocols,
            channel_config,
            variant_configuration,
            variant_files,
            preferred_build_sources,
            source_timestamp_hints,
        }
    }

    pub async fn collect(
        self,
        specs: impl IntoIterator<Item = (rattler_conda_types::PackageName, SourceSpec)>,
    ) -> Result<CollectedSourceMetadata, CommandDispatcherError<CollectSourceMetadataError>> {
        let mut source_futures = CancellationAwareFutures::new(self.command_queue.executor());
        let mut specs = specs
            .into_iter()
            .map(|(name, spec)| (name, spec, Vec::new()))
            .collect::<Vec<_>>();
        let mut result = CollectedSourceMetadata::default();
        let mut already_encountered_specs = HashSet::new();
        let mut collected_errors: Vec<CollectSourceMetadataError> = Vec::new();

        loop {
            // Create futures for all encountered specs.
            for (name, spec, chain) in specs.drain(..) {
                if already_encountered_specs.insert((name.clone(), spec.location.clone())) {
                    source_futures.push(
                        self.collect_source_metadata(name, spec, chain)
                            .boxed_local(),
                    );
                }
            }

            // Wait for the next future to finish. Cancelled results are
            // transparently skipped by the `CancellationAwareFutures` adapter.
            // Only real errors or successes arrive here.
            let Some(source_metadata) = source_futures.next().await else {
                // No more pending futures, we are done.
                if let Some(err) = collected_errors.into_iter().next() {
                    return Err(CommandDispatcherError::Failed(err));
                }
                return Ok(result);
            };

            // Collect errors but let remaining futures complete.
            let (source_metadata, mut chain) = match source_metadata {
                Ok(v) => v,
                Err(CommandDispatcherError::Cancelled) => {
                    return Err(CommandDispatcherError::Cancelled);
                }
                Err(CommandDispatcherError::Failed(err)) => {
                    collected_errors.push(err);
                    continue;
                }
            };

            // Process transitive dependencies
            for record in &source_metadata.records {
                chain.push(record.package_record().name.clone());
                let anchor =
                    SourceAnchor::from(SourceLocationSpec::from(record.manifest_source().clone()));
                for depend in &record.package_record().depends {
                    if let Ok(spec) = MatchSpec::from_str(depend, ParseStrictness::Lenient) {
                        let (PackageNameMatcher::Exact(name), nameless_spec) =
                            spec.clone().into_nameless()
                        else {
                            unimplemented!(
                                "non exact packages names are not supported in {depend}"
                            );
                        };
                        if let Some(source_location) = record.sources().get(name.as_normalized()) {
                            // We encountered a transitive source dependency.
                            let resolved_location = anchor.resolve(source_location.clone());
                            specs.push((
                                name,
                                SourceSpec::new(resolved_location, nameless_spec),
                                chain.clone(),
                            ));
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
        tracing::trace!("Collecting source metadata for {}", name.as_source());

        // Determine if we should override the build_source pin for this package.
        let preferred_build_source = self.preferred_build_sources.get(&name).cloned();
        // Always checkout the manifest-defined source location (root), discovery
        // will pick build_source; we only pass preferred locations.
        let manifest_source_checkout = self
            .command_queue
            .pin_and_checkout(spec.location)
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
                    manifest_source: manifest_source_checkout.pinned,
                    preferred_build_source,
                    channel_config: self.channel_config.clone(),
                    channels: self.channels.clone(),
                    build_environment: self.build_environment.clone(),
                    exclude_newer: self.exclude_newer.clone(),
                    variant_configuration: self.variant_configuration.clone(),
                    variant_files: self.variant_files.clone(),
                    enabled_protocols: self.enabled_protocols.clone(),
                },
                exclude_newer: self.exclude_newer.clone(),
                source_exclude_newer_hints: self.source_timestamp_hints.clone(),
            })
            .await
        {
            Err(CommandDispatcherError::Cancelled) => {
                return Err(CommandDispatcherError::Cancelled);
            }
            Err(CommandDispatcherError::Failed(SourceMetadataError::SourceRecord(
                SourceRecordError::Cycle(mut cycle),
            ))) => {
                // Push the packages that led up to this cycle onto the cycle stack.
                cycle
                    .stack
                    .extend(chain.into_iter().map(|pkg| (pkg, CycleEnvironment::Run)));
                return Err(CommandDispatcherError::Failed(
                    CollectSourceMetadataError::SourceMetadataError {
                        name,
                        error: SourceMetadataError::SourceRecord(SourceRecordError::Cycle(cycle)),
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

        Ok((source_metadata, chain))
    }
}
