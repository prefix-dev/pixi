mod source_metadata_collector;

use std::{path::PathBuf, time::Instant};

use chrono::{DateTime, Utc};
use futures::{TryFutureExt, TryStreamExt, stream::FuturesUnordered};
use itertools::{Either, Itertools};
use miette::Diagnostic;
use pixi_build_frontend::EnabledProtocols;
use pixi_record::PixiRecord;
use pixi_spec::{PixiSpec, SourceSpec};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{Channel, ChannelConfig, ChannelUrl, NamelessMatchSpec, Platform};
use rattler_repodata_gateway::RepoData;
use rattler_solve::{ChannelPriority, SolveStrategy};
use thiserror::Error;

use crate::{
    CommandQueue, CommandQueueError, CondaEnvironmentSpec, SourceMetadataSpec,
    build::BuildEnvironment,
    command_queue::CommandQueueErrorResultExt,
    conda::SolveCondaEnvironmentError,
    pixi::source_metadata_collector::{
        CollectSourceMetadataError, CollectedSourceMetadata, SourceMetadataCollector,
    },
    source_metadata::{SourceMetadata, SourceMetadataError},
};

/// Contains all information that describes the input of a pixi environment.
/// This is very similar to a [`CondaEnvironmentSpec`], but also supports
/// building certain dependencies from source.
#[derive(Debug, Clone)]
pub struct PixiEnvironmentSpec {
    /// The requirements of the environment
    pub requirements: DependencyMap<rattler_conda_types::PackageName, PixiSpec>,

    /// Additional constraints of the environment
    pub constraints: DependencyMap<rattler_conda_types::PackageName, NamelessMatchSpec>,

    /// The records of the packages that are currently already installed. These
    /// are used as hints to reduce the difference between individual solves.
    pub installed: Vec<PixiRecord>,

    /// The environment that we are solving for
    pub build_environment: BuildEnvironment,

    /// The channels to use for solving
    pub channels: Vec<ChannelUrl>,

    /// The strategy to use for solving
    pub strategy: SolveStrategy,

    /// The priority of channels to use for solving
    pub channel_priority: ChannelPriority,

    /// Exclude any packages after the first cut-off date.
    pub exclude_newer: Option<DateTime<Utc>>,

    /// The channel configuration to use for this environment.
    pub channel_config: ChannelConfig,

    /// The protocols that are enabled for source packages
    pub enabled_protocols: EnabledProtocols,
}

impl Default for PixiEnvironmentSpec {
    fn default() -> Self {
        Self {
            requirements: Default::default(),
            constraints: Default::default(),
            installed: Vec::new(),
            build_environment: BuildEnvironment::default(),
            channels: vec![],
            strategy: SolveStrategy::default(),
            channel_priority: ChannelPriority::Strict,
            exclude_newer: None,
            channel_config: ChannelConfig::default_with_root_dir(PathBuf::from(".")),
            enabled_protocols: EnabledProtocols::default(),
        }
    }
}

impl PixiEnvironmentSpec {
    /// Solves this environment using the given command_queue.
    pub async fn solve(
        self,
        command_queue: CommandQueue,
    ) -> Result<Vec<PixiRecord>, CommandQueueError<SolvePixiEnvironmentError>> {
        // Split the requirements into source and binary requirements.
        let (source_specs, binary_specs) = Self::split_into_source_and_binary_requirements(
            &self.channel_config,
            self.requirements,
        );

        // Recursively collect the metadata of all the source specs.
        let CollectedSourceMetadata {
            source_repodata,
            transitive_dependencies,
        } = SourceMetadataCollector::new(
            command_queue.clone(),
            self.channels.clone(),
            self.channel_config.clone(),
            self.build_environment.clone(),
            self.enabled_protocols.clone(),
        )
        .collect(
            source_specs
                .iter_specs()
                .map(|(name, spec)| (name.clone(), spec.clone()))
                .collect(),
        )
        .await
        .map_err_with(SolvePixiEnvironmentError::from)?;

        // Query the gateway for conda repodata.
        let fetch_repodata_start = Instant::now();
        let available_records = command_queue
            .gateway()
            .query(
                self.channels.into_iter().map(Channel::from_url),
                [self.build_environment.host_platform, Platform::NoArch],
                binary_specs
                    .iter_match_specs()
                    .chain(transitive_dependencies),
            )
            .recursive(true)
            .await
            .map_err(SolveCondaEnvironmentError::QueryError)
            .map_err(SolvePixiEnvironmentError::SolveCondaEnvironmentError)?;

        let total_records = available_records.iter().map(RepoData::len).sum::<usize>();
        tracing::info!(
            "fetched {total_records} records in {:?}",
            fetch_repodata_start.elapsed()
        );

        // Filter all installed packages
        let installed = self
            .installed
            .into_iter()
            // Only lock binary records
            .filter_map(|record| record.into_binary())
            // Filter any record we want as a source record
            .filter(|record| !source_specs.contains_key(&record.package_record.name))
            .collect();

        // Solve the conda environment
        let solver_result = command_queue
            .solve_conda_environment(CondaEnvironmentSpec {
                requirements: binary_specs,
                constraints: self.constraints,
                installed,
                platform: self.build_environment.host_platform,
                channels: self.channels,
                virtual_packages: self.build_environment.host_virtual_packages,
                strategy: self.strategy,
                channel_priority: self.channel_priority,
                exclude_newer: self.exclude_newer,
                channel_config: self.channel_config,
            })
            .await
            .map_err_with(SolvePixiEnvironmentError::from)?;

        // Convert the result back into the pixi records.
        Ok(solver_result.into_iter().map(PixiRecord::Binary).collect())
    }

    /// Split the set of requirements into source and binary requirements.
    ///
    /// This method doesn't take `self` so we can move ownership of
    /// [`Self::requirements`] without also taking a mutable reference to
    /// `self`.
    fn split_into_source_and_binary_requirements(
        channel_config: &ChannelConfig,
        specs: DependencyMap<rattler_conda_types::PackageName, PixiSpec>,
    ) -> (
        DependencyMap<rattler_conda_types::PackageName, SourceSpec>,
        DependencyMap<rattler_conda_types::PackageName, NamelessMatchSpec>,
    ) {
        specs.into_specs().partition_map(|(name, constraint)| {
            match constraint.into_source_or_binary() {
                Either::Left(source) => Either::Left((name, source)),
                Either::Right(binary) => {
                    let spec = binary
                        .try_into_nameless_match_spec(&channel_config)
                        .expect("failed to convert channel from spec");
                    Either::Right((name, spec))
                }
            }
        })
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum SolvePixiEnvironmentError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    SolveCondaEnvironmentError(#[from] SolveCondaEnvironmentError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    CollectSourceMetadataError(#[from] CollectSourceMetadataError),
}
