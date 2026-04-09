pub(crate) mod cycle;

use crate::{
    BuildBackendMetadataError, BuildBackendMetadataSpec, CommandDispatcher, CommandDispatcherError,
    CommandDispatcherErrorResultExt, PackageNotProvidedError,
    build::PinnedSourceCodeLocation,
    executor::CancellationAwareFutures,
    source_record::{SourceRecordError, SourceRecordSpec},
};
pub use cycle::{Cycle, CycleEnvironment};
use miette::Diagnostic;
use pixi_record::{SourceRecord, SourceRecordReuseKey, SourceTimestamps, VariantValue};
use pixi_spec::ResolvedExcludeNewer;
use rattler_conda_types::PackageName;
use std::collections::HashMap;
use thiserror::Error;
use tracing::instrument;

#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceMetadataSpec {
    /// The name of the package to retrieve metadata from.
    pub package: PackageName,

    /// Information about the build backend to request the information from.
    pub backend_metadata: BuildBackendMetadataSpec,

    /// The timestamp exclusion to apply when retrieving the metadata.
    pub exclude_newer: Option<ResolvedExcludeNewer>,

    /// Exclude-newer hints keyed by exact source output identity. Used to
    /// soft-lock build/host dependencies when re-resolving.
    #[serde(skip)]
    pub source_exclude_newer_hints: HashMap<SourceRecordReuseKey, SourceTimestamps>,
}

/// The result of resolving source metadata for all variants of a package.
#[derive(Debug)]
pub struct SourceMetadata {
    /// Manifest and optional build source location for this metadata.
    pub source: PinnedSourceCodeLocation,

    /// The metadata that was acquired from the build backend.
    pub records: Vec<SourceRecord>,
}

impl SourceMetadataSpec {
    #[instrument(
        skip_all,
        name = "source-metadata",
        fields(
            manifest_source= %self.backend_metadata.manifest_source,
            preferred_build_source=self.backend_metadata.preferred_build_source.as_ref().map(tracing::field::display),
            name = %self.package.as_source(),
            platform = %self.backend_metadata.build_environment.host_platform,
        )
    )]
    pub(crate) async fn request(
        self,
        command_dispatcher: CommandDispatcher,
    ) -> Result<SourceMetadata, CommandDispatcherError<SourceMetadataError>> {
        // Get the metadata from the build backend.
        let build_backend_metadata = command_dispatcher
            .build_backend_metadata(self.backend_metadata.clone())
            .await
            .map_err_with(SourceMetadataError::BuildBackendMetadata);

        let build_backend_metadata = build_backend_metadata?;

        tracing::trace!(
            "Retrieving source metadata for package {}",
            self.package.as_source()
        );

        // Find all outputs matching the requested package name.
        let mut matching_outputs = build_backend_metadata
            .metadata
            .outputs
            .iter()
            .filter(|o| o.metadata.name == self.package)
            .peekable();

        if matching_outputs.peek().is_none() {
            let available_names = build_backend_metadata
                .metadata
                .outputs
                .iter()
                .map(|output| output.metadata.name.clone());
            return Err(CommandDispatcherError::Failed(
                PackageNotProvidedError::new(
                    self.package,
                    build_backend_metadata.source.manifest_source().clone(),
                    available_names,
                )
                .into(),
            ));
        }

        // Fan out a SourceRecordSpec for each matching output variant concurrently.
        let mut futures = CancellationAwareFutures::new(command_dispatcher.executor());
        for output in matching_outputs {
            let variants: std::collections::BTreeMap<String, VariantValue> = output
                .metadata
                .variant
                .iter()
                .map(|(k, v)| (k.clone(), VariantValue::from(v.clone())))
                .collect();

            // Take the resolved exclude-newer and constrain it further with
            // timestamp hints from a previous solve if available.
            let key = SourceRecordReuseKey::new(self.package.clone(), variants.clone());
            let exclude_newer = match (
                self.exclude_newer.clone(),
                self.source_exclude_newer_hints.get(&key),
            ) {
                (Some(en), Some(hint)) => Some(en.constraint_to_timestamps(hint)),
                (en, _) => en,
            };

            let dispatcher = command_dispatcher.clone();
            let spec = SourceRecordSpec {
                package: self.package.clone(),
                variants,
                backend_metadata: self.backend_metadata.clone(),
                exclude_newer,
            };
            futures.push(async move {
                dispatcher
                    .source_record(spec)
                    .await
                    .map_err_with(SourceMetadataError::SourceRecord)
            });
        }

        let (resolved, errors) = futures.collect_all().await?;

        // If any source record resolutions failed, return the first error.
        // All tasks ran to completion so the user sees all side effects.
        if let Some(err) = errors.into_iter().next() {
            return Err(CommandDispatcherError::Failed(err));
        }

        let records = resolved.iter().map(|r| r.record.clone()).collect();

        Ok(SourceMetadata {
            source: build_backend_metadata.source.clone(),
            records,
        })
    }
}

#[derive(Debug, Clone, Error, Diagnostic)]
pub enum SourceMetadataError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    BuildBackendMetadata(#[from] BuildBackendMetadataError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    SourceRecord(#[from] SourceRecordError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    PackageNotProvided(#[from] PackageNotProvidedError),
}
