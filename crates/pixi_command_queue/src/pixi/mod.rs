use std::path::PathBuf;

use chrono::{DateTime, Utc};
use itertools::{Either, Itertools};
use miette::Diagnostic;
use pixi_record::PixiRecord;
use pixi_spec::{PixiSpec, SourceSpec};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{
    ChannelConfig, ChannelUrl, GenericVirtualPackage, NamelessMatchSpec, Platform,
};
use rattler_solve::{ChannelPriority, SolveStrategy};
use thiserror::Error;

use crate::{
    CommandQueue, CommandQueueError, CondaEnvironmentSpec,
    command_queue::CommandQueueErrorResultExt, conda::SolveCondaEnvironmentError,
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

impl Default for PixiEnvironmentSpec {
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

impl PixiEnvironmentSpec {
    /// Solves this environment using the given command_queue.
    pub async fn solve(
        self,
        dispatcher: CommandQueue,
    ) -> Result<Vec<PixiRecord>, CommandQueueError<SolvePixiEnvironmentError>> {
        // Split the requirements into source and binary requirements.
        let (source_specs, binary_specs) = Self::split_into_source_and_binary_requirements(
            &self.channel_config,
            self.requirements,
        );

        // Iterate over all source specs and get their metadata.
        for source in source_specs.iter_specs() {}

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
        let solver_result = dispatcher
            .solve_conda_environment(CondaEnvironmentSpec {
                requirements: binary_specs,
                constraints: self.constraints,
                installed,
                platform: self.platform,
                channels: self.channels,
                virtual_packages: self.virtual_packages,
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
    SolveCondaEnvironmentError(#[from] SolveCondaEnvironmentError),
}
