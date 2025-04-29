use std::{path::PathBuf, time::Instant};

use chrono::{DateTime, Utc};
use itertools::{Either, Itertools};
use miette::Diagnostic;
use pixi_record::PixiRecord;
use pixi_spec::{BinarySpec, PixiSpec, SourceSpec};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{
    Channel, ChannelConfig, ChannelUrl, GenericVirtualPackage, MatchSpec, NamelessMatchSpec,
    Platform, RepoDataRecord,
};
use rattler_repodata_gateway::RepoData;
use rattler_solve::{ChannelPriority, SolveStrategy, SolverImpl};
use thiserror::Error;
use tokio::task::JoinError;

use crate::{CommandQueue, CommandQueueError};

/// Contains all information that describes the input of a conda environment.
#[derive(Debug, Clone)]
pub struct CondaEnvironmentSpec {
    /// The requirements of the environment
    pub requirements: DependencyMap<rattler_conda_types::PackageName, PixiSpec>,

    /// Additional constraints of the environment
    pub constraints: DependencyMap<rattler_conda_types::PackageName, NamelessMatchSpec>,

    /// The records of the packages that are currently already installed. These
    /// are used as hints to reduce the difference between individual solves.
    pub installed: Vec<PixiRecord>,

    /// The platform to solve for
    pub platform: Platform,

    /// The channels to use for solving
    pub channels: Vec<ChannelUrl>,

    /// The virtual packages to include in the solve
    pub virtual_packages: Vec<GenericVirtualPackage>,

    /// The strategy to use for solving
    pub strategy: SolveStrategy,

    /// The priority of channels to use for solving
    pub channel_priority: ChannelPriority,

    /// Exclude any packages after the first cut-off date.
    pub exclude_newer: Option<DateTime<Utc>>,

    /// The channel configuration to use for this environment.
    pub channel_config: ChannelConfig,
}

impl Default for CondaEnvironmentSpec {
    fn default() -> Self {
        Self {
            requirements: Default::default(),
            constraints: Default::default(),
            installed: Vec::new(),
            platform: Platform::current(),
            channels: vec![],
            virtual_packages: vec![],
            strategy: SolveStrategy::default(),
            channel_priority: ChannelPriority::Strict,
            exclude_newer: None,
            channel_config: ChannelConfig::default_with_root_dir(PathBuf::from(".")),
        }
    }
}

impl CondaEnvironmentSpec {
    /// Solves this environment using the given command_queue.
    pub async fn solve(
        self,
        dispatcher: CommandQueue,
    ) -> Result<Vec<PixiRecord>, CommandQueueError<SolveCondaEnvironmentError>> {
        // Split the requirements into source and binary requirements.
        let (source_specs, binary_specs) = Self::split_into_source_and_binary_requirements(
            &self.channel_config,
            self.requirements,
        );

        // Iterate over all source specs and get their metadata.
        for source in source_specs.iter_specs() {}

        // Query the gateway for conda repodata.
        let fetch_repodata_start = Instant::now();
        let available_records = dispatcher
            .gateway()
            .query(
                self.channels.into_iter().map(Channel::from_url),
                [self.platform, Platform::NoArch],
                binary_specs.iter_match_specs(),
            )
            .recursive(true)
            .await
            .map_err(SolveCondaEnvironmentError::QueryError)?;

        let total_records = available_records.iter().map(RepoData::len).sum::<usize>();
        tracing::info!(
            "fetched {total_records} records in {:?}",
            fetch_repodata_start.elapsed()
        );

        // Filter all installed packages
        let locked_packages = self
            .installed
            .into_iter()
            // Only lock binary records
            .filter_map(|record| record.into_binary())
            // Filter any record we want as a source record
            .filter(|record| !source_specs.contains_key(&record.package_record.name))
            .collect();

        // Solving is a CPU-intensive task, we spawn this on a background task to allow
        // for more concurrency.
        let solve_result = tokio::task::spawn_blocking(move || {
            // Construct a task to solve the environment.
            let task = rattler_solve::SolverTask {
                specs: binary_specs.into_match_specs().collect(),
                locked_packages,
                virtual_packages: self.virtual_packages,
                channel_priority: self.channel_priority,
                exclude_newer: self.exclude_newer,
                strategy: self.strategy,
                ..rattler_solve::SolverTask::from_iter(&available_records)
            };

            rattler_solve::resolvo::Solver.solve(task)
        })
        .await;

        // Error out if the background task failed or was canceled.
        let solver_result = match solve_result.map_err(JoinError::try_into_panic) {
            Err(Err(_)) => return Err(CommandQueueError::Cancelled),
            Err(Ok(panic)) => std::panic::resume_unwind(panic),
            Ok(Err(err)) => {
                return Err(CommandQueueError::Failed(err.into()));
            }
            Ok(Ok(result)) => result,
        };

        // Convert the result back into the pixi records.
        Ok(solver_result
            .records
            .into_iter()
            .map(PixiRecord::Binary)
            .collect())
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
pub enum SolveCondaEnvironmentError {
    #[error(transparent)]
    QueryError(#[from] rattler_repodata_gateway::GatewayError),

    #[error("failed to solve the conda environment")]
    SolveError(#[from] rattler_solve::SolveError),
}
