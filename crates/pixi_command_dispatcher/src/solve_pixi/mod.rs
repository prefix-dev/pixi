mod reporter;
mod source_metadata_collector;

use std::{borrow::Borrow, collections::BTreeMap, path::PathBuf, time::Instant};

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use itertools::{Either, Itertools};
use miette::Diagnostic;
use pixi_build_discovery::EnabledProtocols;
use pixi_record::PixiRecord;
use pixi_spec::{BinarySpec, PixiSpec, SourceSpec, SpecConversionError};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{Channel, ChannelConfig, ChannelUrl, ParseChannelError, Platform};
use rattler_repodata_gateway::RepoData;
use rattler_solve::{ChannelPriority, SolveStrategy};
use reporter::WrappingGatewayReporter;
use serde::Serialize;
use thiserror::Error;
use tracing::instrument;

use crate::{
    BuildEnvironment, CommandDispatcher, CommandDispatcherError, CommandDispatcherErrorResultExt,
    Cycle, SolveCondaEnvironmentSpec, SourceMetadataError,
    solve_conda::SolveCondaEnvironmentError,
    solve_pixi::source_metadata_collector::{
        CollectSourceMetadataError, CollectedSourceMetadata, SourceMetadataCollector,
    },
};

/// Contains all information that describes the input of a pixi environment.
///
/// Information about binary packages is requested as part of solving this
/// instance.
///
/// When solving a pixi environment, source records are checked out and their
/// metadata is queried. This may involve a recursive pattern of solving if the
/// sources require additional environments to be set up.
///
/// If all the input information is already available and no recursion is
/// desired, use [`SolveCondaEnvironmentSpec`] instead.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct PixiEnvironmentSpec {
    pub name: Option<String>,

    /// The requirements of the environment
    #[serde(skip_serializing_if = "DependencyMap::is_empty")]
    pub dependencies: DependencyMap<rattler_conda_types::PackageName, PixiSpec>,

    /// Additional constraints of the environment
    #[serde(skip_serializing_if = "DependencyMap::is_empty")]
    pub constraints: DependencyMap<rattler_conda_types::PackageName, BinarySpec>,

    /// Development sources whose dependencies should be installed without
    /// building the packages themselves.
    #[serde(skip_serializing_if = "IndexMap::is_empty")]
    pub dev_sources: IndexMap<rattler_conda_types::PackageName, pixi_spec::DevSourceSpec>,

    /// The records of the packages that are currently already installed. These
    /// are used as hints to reduce the difference between individual solves.
    #[serde(skip)]
    pub installed: Vec<PixiRecord>,

    /// The environment that we are solving for
    pub build_environment: BuildEnvironment,

    /// The channels to use for solving
    pub channels: Vec<ChannelUrl>,

    /// The strategy to use for solving
    #[serde(skip_serializing_if = "crate::is_default")]
    pub strategy: SolveStrategy,

    /// The priority of channels to use for solving
    #[serde(skip_serializing_if = "crate::is_default")]
    pub channel_priority: ChannelPriority,

    /// Exclude any packages after the first cut-off date.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude_newer: Option<DateTime<Utc>>,

    /// The channel configuration to use for this environment.
    pub channel_config: ChannelConfig,

    /// Build variants to use during the solve
    pub variants: Option<BTreeMap<String, Vec<String>>>,

    /// Variant file paths to use during the solve
    pub variant_files: Option<Vec<PathBuf>>,

    /// The protocols that are enabled for source packages
    #[serde(skip_serializing_if = "crate::is_default")]
    pub enabled_protocols: EnabledProtocols,
}

impl Default for PixiEnvironmentSpec {
    fn default() -> Self {
        Self {
            name: None,
            dependencies: DependencyMap::default(),
            constraints: DependencyMap::default(),
            dev_sources: IndexMap::new(),
            installed: Vec::new(),
            build_environment: BuildEnvironment::default(),
            channels: vec![],
            strategy: SolveStrategy::default(),
            channel_priority: ChannelPriority::Strict,
            exclude_newer: None,
            channel_config: ChannelConfig::default_with_root_dir(PathBuf::from(".")),
            variants: None,
            variant_files: None,
            enabled_protocols: EnabledProtocols::default(),
        }
    }
}

impl PixiEnvironmentSpec {
    /// Solves this environment using the given command_dispatcher.
    #[instrument(
        skip_all,
        fields(
            name = self.name.as_deref().unwrap_or("unspecified"),
            platform = %self.build_environment.host_platform,
        )
    )]
    pub async fn solve(
        self,
        command_queue: CommandDispatcher,
        gateway_reporter: Option<Box<dyn rattler_repodata_gateway::Reporter>>,
    ) -> Result<Vec<PixiRecord>, CommandDispatcherError<SolvePixiEnvironmentError>> {
        // Process dev sources to get their metadata (before dependencies are moved)
        let dev_source_records = self.process_dev_sources(&command_queue).await?;

        // Split the requirements into source and binary requirements.
        let (dev_source_source_specs, dev_source_binary_specs) =
            Self::split_into_source_and_binary_requirements(Self::dev_source_dependencies(
                &dev_source_records,
            ));
        let (source_specs, binary_specs) =
            Self::split_into_source_and_binary_requirements(self.dependencies.into_specs());

        Self::check_missing_channels(binary_specs.clone(), &self.channels, &self.channel_config)?;

        // Recursively collect the metadata of all the source specs.
        let CollectedSourceMetadata {
            source_repodata,
            transitive_dependencies,
        } = SourceMetadataCollector::new(
            command_queue.clone(),
            self.channels.clone(),
            self.channel_config.clone(),
            self.build_environment.clone(),
            self.variants.clone(),
            self.variant_files.clone(),
            self.enabled_protocols.clone(),
        )
        .collect(
            source_specs
                .iter_specs()
                .map(|(name, spec)| (name.clone(), spec.clone()))
                .chain(dev_source_source_specs.into_specs()),
        )
        .await
        .map_err_with(SolvePixiEnvironmentError::from)?;

        // Convert the binary specs into match specs as well.
        let binary_match_specs = binary_specs
            .clone()
            .into_match_specs(&self.channel_config)
            .map_err(SolvePixiEnvironmentError::SpecConversionError)
            .map_err(CommandDispatcherError::Failed)?;

        let dev_source_binary_match_specs = dev_source_binary_specs
            .into_match_specs(&self.channel_config)
            .map_err(SolvePixiEnvironmentError::SpecConversionError)
            .map_err(CommandDispatcherError::Failed)?;

        // Query the gateway for conda repodata. This fetches the repodata for both the
        // direct dependencies of the environment and the direct dependencies of
        // all (recursively) discovered source dependencies. This ensures that all
        // repodata required to solve the environment is loaded.
        let fetch_repodata_start = Instant::now();
        let query = command_queue
            .gateway()
            .query(
                self.channels.iter().cloned().map(Channel::from_url),
                [self.build_environment.host_platform, Platform::NoArch],
                binary_match_specs
                    .into_iter()
                    .chain(transitive_dependencies)
                    .chain(dev_source_binary_match_specs),
            )
            .recursive(true);

        let query = if let Some(gateway_reporter) = gateway_reporter {
            query.with_reporter(WrappingGatewayReporter(gateway_reporter))
        } else {
            query
        };

        let binary_repodata = query
            .await
            .map_err(SolvePixiEnvironmentError::QueryError)
            .map_err(CommandDispatcherError::Failed)?;
        let total_records = binary_repodata.iter().map(RepoData::len).sum::<usize>();
        tracing::debug!(
            "fetched {total_records} records in {:?}",
            fetch_repodata_start.elapsed()
        );

        // Construct a solver specification from the collected metadata and solve the
        // environment.
        command_queue
            .solve_conda_environment(SolveCondaEnvironmentSpec {
                name: self.name,
                source_specs,
                binary_specs,
                constraints: self.constraints,
                dev_source_records,
                source_repodata,
                binary_repodata,
                installed: self.installed,
                platform: self.build_environment.host_platform,
                channels: self.channels,
                virtual_packages: self.build_environment.host_virtual_packages,
                strategy: self.strategy,
                channel_priority: self.channel_priority,
                exclude_newer: self.exclude_newer,
                channel_config: self.channel_config,
            })
            .await
            .map_err_with(SolvePixiEnvironmentError::from)
    }

    /// Process dev sources to retrieve their metadata and create DevSourceRecords.
    ///
    /// For each dev source, this method:
    /// 1. Pins and checks out the source
    /// 2. Queries the build backend for metadata
    /// 3. Creates DevSourceRecords for matching outputs
    async fn process_dev_sources(
        &self,
        command_dispatcher: &CommandDispatcher,
    ) -> Result<Vec<pixi_record::DevSourceRecord>, CommandDispatcherError<SolvePixiEnvironmentError>>
    {
        use crate::{BuildBackendMetadataSpec, DevSourceMetadataSpec};
        use futures::StreamExt;

        let mut dev_source_futures =
            crate::executor::ExecutorFutures::new(command_dispatcher.executor());

        // Create a future for each dev source
        for (package_name, dev_source_spec) in &self.dev_sources {
            let command_dispatcher = command_dispatcher.clone();
            let package_name = package_name.clone();
            let dev_source_spec = dev_source_spec.clone();
            let channel_config = self.channel_config.clone();
            let channels = self.channels.clone();
            let build_environment = self.build_environment.clone();
            let variants = self.variants.clone();
            let variant_files = self.variant_files.clone();
            let enabled_protocols = self.enabled_protocols.clone();

            dev_source_futures.push(async move {
                // Pin and checkout the source
                let pinned_source = command_dispatcher
                    .pin_and_checkout(dev_source_spec.source)
                    .await
                    .map_err_with(SolvePixiEnvironmentError::SourceCheckoutError)?;

                // Create the spec for getting dev source metadata
                let spec = DevSourceMetadataSpec {
                    package_name,
                    backend_metadata: BuildBackendMetadataSpec {
                        source: pinned_source.pinned,
                        channel_config,
                        channels,
                        build_environment,
                        variants,
                        variant_files,
                        enabled_protocols,
                    },
                };

                // Get the dev source metadata
                command_dispatcher
                    .dev_source_metadata(spec)
                    .await
                    .map_err_with(SolvePixiEnvironmentError::DevSourceMetadataError)
            });
        }

        // Collect all dev source records
        let mut all_records = Vec::new();
        while let Some(result) = dev_source_futures.next().await {
            let metadata = result?;
            all_records.extend(metadata.records);
        }

        Ok(all_records)
    }

    /// Returns an iterator over all dependencies from dev source records,
    /// excluding packages that are themselves dev sources.
    fn dev_source_dependencies<'a>(
        dev_source_records: &'a [pixi_record::DevSourceRecord],
    ) -> impl Iterator<Item = (rattler_conda_types::PackageName, PixiSpec)> + 'a {
        use std::collections::HashSet;

        // Collect all dev source package names to filter them out
        let dev_source_names: HashSet<_> = dev_source_records
            .iter()
            .map(|record| record.name.clone())
            .collect();

        // Collect all dependencies from all dev sources, filtering out dev sources themselves
        dev_source_records
            .iter()
            .flat_map(|dev_source| {
                dev_source
                    .dependencies
                    .iter_specs()
                    .map(|(name, spec)| (name.clone(), spec.clone()))
                    .collect::<Vec<_>>()
            })
            .filter(move |(name, _)| !dev_source_names.contains(name))
    }

    /// Split the set of requirements into source and binary requirements.
    ///
    /// This method doesn't take `self` so we can move ownership of
    /// [`Self::requirements`] without also taking a mutable reference to
    /// `self`.
    fn split_into_source_and_binary_requirements(
        specs: impl IntoIterator<Item = (rattler_conda_types::PackageName, PixiSpec)>,
    ) -> (
        DependencyMap<rattler_conda_types::PackageName, SourceSpec>,
        DependencyMap<rattler_conda_types::PackageName, BinarySpec>,
    ) {
        specs.into_iter().partition_map(|(name, constraint)| {
            match constraint.into_source_or_binary() {
                Either::Left(source) => Either::Left((name, source)),
                Either::Right(binary) => Either::Right((name, binary)),
            }
        })
    }

    /// Check that binary specs do not refer to inaccessible channels
    fn check_missing_channels(
        binary_specs: DependencyMap<rattler_conda_types::PackageName, BinarySpec>,
        channels: &[ChannelUrl],
        channel_config: &ChannelConfig,
    ) -> Result<(), CommandDispatcherError<SolvePixiEnvironmentError>> {
        for (pkg, spec) in binary_specs.iter_specs() {
            let BinarySpec::DetailedVersion(v) = spec else {
                continue;
            };
            let Some(channel) = &v.channel else { continue };

            let base_url = channel
                .clone()
                .into_base_url(channel_config)
                .map_err(SolvePixiEnvironmentError::ParseChannelError)
                .map_err(CommandDispatcherError::Failed)?;

            if !channels.iter().any(|c| c == &base_url) {
                return Err(CommandDispatcherError::Failed(
                    SolvePixiEnvironmentError::MissingChannel(MissingChannelError {
                        package: pkg.as_normalized().to_string(),
                        channel: base_url,
                        advice: None,
                    }),
                ));
            }
        }
        Ok(())
    }
}

/// An error that might be returned when solving a pixi environment.
#[derive(Debug, Error, Diagnostic)]
pub enum SolvePixiEnvironmentError {
    #[error(transparent)]
    QueryError(#[from] rattler_repodata_gateway::GatewayError),

    #[error("failed to solve the environment")]
    SolveError(#[from] rattler_solve::SolveError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    CollectSourceMetadataError(CollectSourceMetadataError),

    #[error(transparent)]
    SpecConversionError(#[from] SpecConversionError),

    #[error("detected a cyclic dependency:\n\n{0}")]
    Cycle(Cycle),

    #[error(transparent)]
    ParseChannelError(#[from] ParseChannelError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    MissingChannel(MissingChannelError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    DevSourceMetadataError(crate::DevSourceMetadataError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    SourceCheckoutError(crate::SourceCheckoutError),
}

/// An error for a missing channel in the solve request
#[derive(Debug, Diagnostic, Error)]
#[error("Package '{package}' requested unavailable channel '{channel}'")]
pub struct MissingChannelError {
    pub package: String,
    pub channel: ChannelUrl,
    #[help]
    pub advice: Option<String>,
}

impl Borrow<dyn Diagnostic> for Box<SolvePixiEnvironmentError> {
    fn borrow(&self) -> &(dyn Diagnostic + 'static) {
        self.as_ref()
    }
}

impl From<SolveCondaEnvironmentError> for SolvePixiEnvironmentError {
    fn from(err: SolveCondaEnvironmentError) -> Self {
        match err {
            SolveCondaEnvironmentError::SolveError(err) => {
                SolvePixiEnvironmentError::SolveError(err)
            }
            SolveCondaEnvironmentError::SpecConversionError(err) => {
                SolvePixiEnvironmentError::SpecConversionError(err)
            }
        }
    }
}

impl From<CollectSourceMetadataError> for SolvePixiEnvironmentError {
    fn from(err: CollectSourceMetadataError) -> Self {
        match err {
            CollectSourceMetadataError::SourceMetadataError {
                error: SourceMetadataError::Cycle(cycle),
                ..
            } => SolvePixiEnvironmentError::Cycle(cycle),
            _ => SolvePixiEnvironmentError::CollectSourceMetadataError(err),
        }
    }
}

impl From<crate::DevSourceMetadataError> for SolvePixiEnvironmentError {
    fn from(err: crate::DevSourceMetadataError) -> Self {
        Self::DevSourceMetadataError(err)
    }
}
