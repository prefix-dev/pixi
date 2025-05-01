use std::{path::PathBuf, time::Instant};

use chrono::{DateTime, Utc};
use miette::Diagnostic;
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{
    Channel, ChannelConfig, ChannelUrl, GenericVirtualPackage, NamelessMatchSpec, Platform,
    RepoDataRecord,
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
    pub requirements: DependencyMap<rattler_conda_types::PackageName, NamelessMatchSpec>,

    /// Additional constraints of the environment
    pub constraints: DependencyMap<rattler_conda_types::PackageName, NamelessMatchSpec>,

    /// The records of the packages that are currently already installed. These
    /// are used as hints to reduce the difference between individual solves.
    pub installed: Vec<RepoDataRecord>,

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
    ) -> Result<Vec<RepoDataRecord>, CommandQueueError<SolveCondaEnvironmentError>> {
        // Query the gateway for conda repodata.
        let fetch_repodata_start = Instant::now();
        let available_records = dispatcher
            .gateway()
            .query(
                self.channels.into_iter().map(Channel::from_url),
                [self.platform, Platform::NoArch],
                self.requirements.iter_match_specs(),
            )
            .recursive(true)
            .await
            .map_err(SolveCondaEnvironmentError::QueryError)?;

        let total_records = available_records.iter().map(RepoData::len).sum::<usize>();
        tracing::info!(
            "fetched {total_records} records in {:?}",
            fetch_repodata_start.elapsed()
        );

        // Solving is a CPU-intensive task, we spawn this on a background task to allow
        // for more concurrency.
        let solve_result = tokio::task::spawn_blocking(move || {
            // Construct a task to solve the environment.
            let task = rattler_solve::SolverTask {
                specs: self.requirements.into_match_specs().collect(),
                locked_packages: self.installed,
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

        Ok(solver_result.records)
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum SolveCondaEnvironmentError {
    #[error(transparent)]
    QueryError(#[from] rattler_repodata_gateway::GatewayError),

    #[error("failed to solve the conda environment")]
    SolveError(#[from] rattler_solve::SolveError),
}
