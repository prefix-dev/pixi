mod reporter;
mod source_metadata_collector;

use std::{borrow::Borrow, collections::BTreeMap, path::PathBuf, time::Instant};

use chrono::{DateTime, Utc};
use itertools::{Either, Itertools};
use miette::Diagnostic;
use pixi_build_discovery::EnabledProtocols;
use pixi_record::{PixiPackageRecord, PixiRecord};
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

    /// Optional override for a specific packages: use this pinned
    /// source for checkout and as the `package_build_source` instead
    /// of pinning anew.
    #[serde(skip)]
    pub pin_overrides: BTreeMap<rattler_conda_types::PackageName, pixi_record::PinnedSourceSpec>,
}

impl Default for PixiEnvironmentSpec {
    fn default() -> Self {
        Self {
            name: None,
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
            variant_files: None,
            enabled_protocols: EnabledProtocols::default(),
            pin_overrides: BTreeMap::new(),
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
    ) -> Result<Vec<PixiPackageRecord>, CommandDispatcherError<SolvePixiEnvironmentError>> {
        // Split the requirements into source and binary requirements.
        let (source_specs, binary_specs) =
            Self::split_into_source_and_binary_requirements(self.dependencies);

        Self::check_missing_channels(binary_specs.clone(), &self.channels, &self.channel_config)
            .map_err(|err| CommandDispatcherError::Failed(*err))?;

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
            self.pin_overrides.clone(),
        )
        .collect(
            source_specs
                .iter_specs()
                .map(|(name, spec)| (name.clone(), spec.clone()))
                .collect(),
        )
        .await
        .map_err_with(SolvePixiEnvironmentError::from)?;

        // Convert the binary specs into match specs as well.
        let binary_match_specs = binary_specs
            .clone()
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
                    .chain(transitive_dependencies),
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

    /// Split the set of requirements into source and binary requirements.
    ///
    /// This method doesn't take `self` so we can move ownership of
    /// [`Self::requirements`] without also taking a mutable reference to
    /// `self`.
    fn split_into_source_and_binary_requirements(
        specs: DependencyMap<rattler_conda_types::PackageName, PixiSpec>,
    ) -> (
        DependencyMap<rattler_conda_types::PackageName, SourceSpec>,
        DependencyMap<rattler_conda_types::PackageName, BinarySpec>,
    ) {
        specs.into_specs().partition_map(|(name, constraint)| {
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
    ) -> Result<(), Box<SolvePixiEnvironmentError>> {
        for (pkg, spec) in binary_specs.iter_specs() {
            if let BinarySpec::DetailedVersion(v) = spec {
                if let Some(channel) = &v.channel {
                    let base_url =
                        channel
                            .clone()
                            .into_base_url(channel_config)
                            .map_err(|err| {
                                Box::new(SolvePixiEnvironmentError::ParseChannelError(err))
                            })?;

                    if !channels.iter().any(|c| c == &base_url) {
                        return Err(Box::new(SolvePixiEnvironmentError::MissingChannel(
                            MissingChannelError {
                                package: pkg.as_normalized().to_string(),
                                channel: base_url,
                                advice: None,
                            },
                        )));
                    }
                }
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
