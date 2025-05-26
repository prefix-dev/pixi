use std::sync::Arc;

use futures::{StreamExt, stream::FuturesUnordered};
use miette::Diagnostic;
use pixi_record::PinnedSourceSpec;
use pixi_spec::{SourceAnchor, SourceSpec};
use rattler_conda_types::{ChannelUrl, MatchSpec, ParseStrictness};
use thiserror::Error;

use crate::build::{BuildContext, BuildEnvironment, BuildError, SourceMetadata};
use crate::reporters::BuildMetadataReporter;

/// An object that is responsible for recursively collecting metadata of source
/// dependencies.
pub struct SourceMetadataCollector {
    build_context: BuildContext,
    channel_urls: Vec<ChannelUrl>,
    build_env: BuildEnvironment,
    metadata_reporter: Arc<dyn BuildMetadataReporter>,
}

#[derive(Default)]
pub struct CollectedSourceMetadata {
    /// Information about all queried source packages. This can be used as
    /// repodata.
    pub source_repodata: Vec<SourceMetadata>,

    /// A list of transitive dependencies of all collected source records.
    pub transitive_dependencies: Vec<MatchSpec>,
}

/// An error that can occur while collecting source metadata.
#[derive(Debug, Error, Diagnostic)]
pub enum ExtractSourceMetadataError {
    #[error("failed to extract metadata for package '{name}'")]
    BuildError {
        name: String,
        #[source]
        #[diagnostic_source]
        error: BuildError,
    },
    #[error("the package '{name}' is not provided by the project located at '{}'", &.pinned_source)]
    PackageMetadataNotFound {
        name: String,
        pinned_source: Box<PinnedSourceSpec>,
        #[help]
        help: String,
    },
}

impl SourceMetadataCollector {
    pub fn new(
        build_context: BuildContext,
        channel_urls: Vec<ChannelUrl>,
        build_env: BuildEnvironment,
        metadata_reporter: Arc<dyn BuildMetadataReporter>,
    ) -> Self {
        Self {
            build_context,
            channel_urls,
            build_env,
            metadata_reporter,
        }
    }

    pub async fn collect(
        self,
        specs: Vec<(rattler_conda_types::PackageName, SourceSpec)>,
    ) -> Result<CollectedSourceMetadata, ExtractSourceMetadataError> {
        let mut source_futures = FuturesUnordered::new();
        let mut specs = specs;
        let mut next_build_id = 0;
        let mut result = CollectedSourceMetadata::default();
        loop {
            // Create futures for all encountered specs.
            for (name, spec) in specs.drain(..) {
                let build_id = next_build_id;
                next_build_id += 1;
                source_futures.push(self.collect_records(name, spec, build_id));
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

    async fn collect_records(
        &self,
        name: rattler_conda_types::PackageName,
        spec: SourceSpec,
        build_id: usize,
    ) -> Result<SourceMetadata, ExtractSourceMetadataError> {
        // Extract information for the particular source spec.
        let source_metadata = self
            .build_context
            .extract_source_metadata(
                &spec,
                &self.channel_urls,
                self.build_env.clone(),
                self.metadata_reporter.clone(),
                build_id,
            )
            .await
            .map_err(|err| ExtractSourceMetadataError::BuildError {
                name: name.as_source().to_string(),
                error: err,
            })?;

        // Make sure that a package with the name defined in spec is available from the
        // backend.
        if !source_metadata
            .records
            .iter()
            .any(|record| record.package_record.name == name)
        {
            return Err(ExtractSourceMetadataError::PackageMetadataNotFound {
                name: name.as_source().to_string(),
                pinned_source: Box::new(source_metadata.source.pinned),
                help: source_metadata
                    .records
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
                    ),
            });
        }

        Ok(source_metadata)
    }
}
