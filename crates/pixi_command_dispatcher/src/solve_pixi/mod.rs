mod source_metadata_collector;

use crate::{
    BuildEnvironment, CommandDispatcher, CommandDispatcherError, CommandDispatcherErrorResultExt,
    SolveCondaEnvironmentSpec,
    solve_pixi::source_metadata_collector::{
        CollectSourceMetadataError, CollectedSourceMetadata, SourceMetadataCollector,
    },
};
use chrono::{DateTime, Utc};
use itertools::{Either, Itertools};
use miette::Diagnostic;
use pixi_build_discovery::EnabledProtocols;
use pixi_record::PixiRecord;
use pixi_spec::{PixiSpec, SourceSpec};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{Channel, ChannelConfig, ChannelUrl, NamelessMatchSpec, Platform};
use rattler_repodata_gateway::RepoData;
use rattler_solve::{ChannelPriority, SolveStrategy};
use serde::Serialize;
use std::collections::BTreeMap;
use std::{path::PathBuf, time::Instant};
use thiserror::Error;

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
    /// The requirements of the environment
    #[serde(skip_serializing_if = "DependencyMap::is_empty")]
    pub dependencies: DependencyMap<rattler_conda_types::PackageName, PixiSpec>,

    /// Additional constraints of the environment
    #[serde(skip_serializing_if = "DependencyMap::is_empty")]
    pub constraints: DependencyMap<rattler_conda_types::PackageName, NamelessMatchSpec>,

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

    /// The protocols that are enabled for source packages
    #[serde(skip_serializing_if = "crate::is_default")]
    pub enabled_protocols: EnabledProtocols,
}

impl Default for PixiEnvironmentSpec {
    fn default() -> Self {
        Self {
            dependencies: DependencyMap::default(),
            constraints: DependencyMap::default(),
            installed: Vec::new(),
            build_environment: BuildEnvironment::default(),
            channels: vec![],
            strategy: SolveStrategy::default(),
            channel_priority: ChannelPriority::Strict,
            exclude_newer: None,
            channel_config: ChannelConfig::default_with_root_dir(PathBuf::from(".")),
            variants: None,
            enabled_protocols: EnabledProtocols::default(),
        }
    }
}

impl PixiEnvironmentSpec {
    /// Solves this environment using the given command_dispatcher.
    pub async fn solve(
        self,
        command_queue: CommandDispatcher,
    ) -> Result<Vec<PixiRecord>, CommandDispatcherError<SolvePixiEnvironmentError>> {
        // Split the requirements into source and binary requirements.
        let (source_specs, binary_specs) = Self::split_into_source_and_binary_requirements(
            &self.channel_config,
            self.dependencies,
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
            self.variants.clone(),
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

        // Query the gateway for conda repodata. This fetches the repodata for both the
        // direct dependencies of the environment and the direct dependencies of
        // all (recursively) discovered source dependencies. This ensures that all
        // repodata required to solve the environment is loaded.
        let fetch_repodata_start = Instant::now();
        let binary_repodata = command_queue
            .gateway()
            .query(
                self.channels.iter().cloned().map(Channel::from_url),
                [self.build_environment.host_platform, Platform::NoArch],
                binary_specs
                    .iter_match_specs()
                    .chain(transitive_dependencies),
            )
            .recursive(true)
            .await
            .map_err(SolvePixiEnvironmentError::QueryError)
            .map_err(CommandDispatcherError::Failed)?;
        let total_records = binary_repodata.iter().map(RepoData::len).sum::<usize>();
        tracing::info!(
            "fetched {total_records} records in {:?}",
            fetch_repodata_start.elapsed()
        );

        // Construct a solver specification from the collected metadata and solve the
        // environment.
        command_queue
            .solve_conda_environment(SolveCondaEnvironmentSpec {
                source_specs,
                binary_specs,
                constraints: self.constraints,
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
            .map_err_with(SolvePixiEnvironmentError::SolveError)
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
                        .try_into_nameless_match_spec(channel_config)
                        .expect("failed to convert channel from spec");
                    Either::Right((name, spec))
                }
            }
        })
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
    CollectSourceMetadataError(#[from] CollectSourceMetadataError),
}
