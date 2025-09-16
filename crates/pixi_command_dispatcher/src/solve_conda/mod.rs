use std::{collections::HashMap, path::PathBuf, sync::Arc};

use chrono::{DateTime, Utc};
use itertools::Itertools;
use pixi_record::{PixiRecord, SourceRecord};
use pixi_spec::{BinarySpec, SourceSpec};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{
    ChannelConfig, ChannelUrl, GenericVirtualPackage, MatchSpec, Platform, RepoDataRecord,
};
use rattler_repodata_gateway::RepoData;
use rattler_solve::{ChannelPriority, SolveStrategy, SolverImpl};
use tokio::task::JoinError;
use url::Url;

use crate::{CommandDispatcherError, SourceMetadata};

/// Contains all information that describes the input of a conda environment.
/// All information about both binary and source packages is stored in the
/// specification, when solving this information is passed to the solver,
/// and the result is returned.
///
/// Unlike [`super::PixiEnvironmentSpec`], solving a `SolveCondaEnvironmentSpec`
/// instance does not require any recursive calls since all information is
/// already available in the specification.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct SolveCondaEnvironmentSpec {
    /// A name, useful for debugging purposes.
    pub name: Option<String>,

    /// Requirements on source packages.
    #[serde(skip_serializing_if = "DependencyMap::is_empty")]
    pub source_specs: DependencyMap<rattler_conda_types::PackageName, SourceSpec>,

    /// Requirements on binary packages.
    #[serde(skip_serializing_if = "DependencyMap::is_empty")]
    pub binary_specs: DependencyMap<rattler_conda_types::PackageName, BinarySpec>,

    /// Additional constraints of the environment
    #[serde(skip_serializing_if = "DependencyMap::is_empty")]
    pub constraints: DependencyMap<rattler_conda_types::PackageName, BinarySpec>,

    /// Available source repodata records.
    #[serde(skip)]
    pub source_repodata: Vec<Arc<SourceMetadata>>,

    /// Available Binary repodata records.
    #[serde(skip)]
    pub binary_repodata: Vec<RepoData>,

    /// The records of the packages that are currently already installed. These
    /// are used as hints to reduce the difference between individual solves.
    #[serde(skip)]
    pub installed: Vec<PixiRecord>,

    /// The platform to solve for
    pub platform: Platform,

    /// The channels to use for solving
    pub channels: Vec<ChannelUrl>,

    /// The virtual packages to include in the solve
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub virtual_packages: Vec<GenericVirtualPackage>,

    /// The strategy to use for solving
    #[serde(skip_serializing_if = "crate::is_default")]
    pub strategy: SolveStrategy,

    /// The priority of channels to use for solving
    #[serde(skip_serializing_if = "crate::is_default")]
    pub channel_priority: ChannelPriority,

    /// Exclude any packages after the first cut-off date.
    #[serde(skip_serializing_if = "crate::is_default")]
    pub exclude_newer: Option<DateTime<Utc>>,

    /// The channel configuration to use for this environment.
    pub channel_config: ChannelConfig,
}

impl Default for SolveCondaEnvironmentSpec {
    fn default() -> Self {
        Self {
            name: None,
            source_specs: DependencyMap::default(),
            binary_specs: DependencyMap::default(),
            constraints: DependencyMap::default(),
            source_repodata: vec![],
            binary_repodata: vec![],
            installed: vec![],
            platform: Platform::current(),
            channels: vec![],
            virtual_packages: vec![],
            strategy: SolveStrategy::default(),
            channel_priority: ChannelPriority::default(),
            exclude_newer: None,
            channel_config: ChannelConfig::default_with_root_dir(PathBuf::from(".")),
        }
    }
}

impl SolveCondaEnvironmentSpec {
    /// Solves this environment
    pub async fn solve(
        self,
    ) -> Result<Vec<PixiRecord>, CommandDispatcherError<SolveCondaEnvironmentError>> {
        // Solving is a CPU-intensive task, we spawn this on a background task to allow
        // for more concurrency.
        let solve_result = tokio::task::spawn_blocking(move || {
            // Filter all installed packages
            let installed = self
                .installed
                .into_iter()
                // Only lock binary records
                .filter_map(|record| record.into_binary())
                // Filter any record we want as a source record
                .filter(|record| !self.source_specs.contains_key(&record.package_record.name))
                .collect();

            // Create direct dependencies on the source packages to feed to the solver.
            let source_match_specs = self
                .source_specs
                .into_specs()
                .map(|(name, spec)| {
                    MatchSpec::from_nameless(spec.to_nameless_match_spec(), Some(name))
                })
                .collect::<Vec<_>>();

            let binary_match_specs = self
                .binary_specs
                .into_match_specs(&self.channel_config)
                .map_err(SolveCondaEnvironmentError::SpecConversionError)?;

            let constrains_match_specs = self
                .constraints
                .into_match_specs(&self.channel_config)
                .map_err(SolveCondaEnvironmentError::SpecConversionError)?;

            // Construct repodata records for source records so that we can feed them to the
            // solver.
            let mut url_to_source_package = HashMap::new();
            for source_metadata in &self.source_repodata {
                for record in &source_metadata.records {
                    let url = unique_url(record);
                    let repodata_record = RepoDataRecord {
                        package_record: record.package_record.clone(),
                        url: url.clone(),
                        file_name: format!(
                            "{}-{}-{}.source",
                            record.package_record.name.as_normalized(),
                            &record.package_record.version,
                            &record.package_record.build
                        ),
                        channel: None,
                    };
                    url_to_source_package.insert(url, (record, repodata_record));
                }
            }

            // Collect repodata records from the remote servers and from the source metadata
            // together. The repodata records go into the first "channel" to ensure
            // they are picked first.
            //
            // TODO: This only holds up when the channel priority is strict. We should
            // probably enforce this better somehow..
            let mut solvable_records = Vec::with_capacity(self.binary_repodata.len() + 1);
            solvable_records.push(
                url_to_source_package
                    .values()
                    .map(|(_, record)| record)
                    .collect_vec(),
            );
            for repo_data in &self.binary_repodata {
                solvable_records.push(repo_data.iter().collect_vec());
            }

            // Construct a solver task that we can start solving.
            let task = rattler_solve::SolverTask {
                specs: source_match_specs
                    .into_iter()
                    .chain(binary_match_specs)
                    .collect(),
                locked_packages: installed,
                virtual_packages: self.virtual_packages,
                channel_priority: self.channel_priority,
                exclude_newer: self.exclude_newer,
                strategy: self.strategy,
                constraints: constrains_match_specs,
                ..rattler_solve::SolverTask::from_iter(solvable_records)
            };

            let solver_result = rattler_solve::resolvo::Solver.solve(task)?;

            // Convert the results back into pixi records.
            Ok::<_, SolveCondaEnvironmentError>(
                solver_result
                    .records
                    .into_iter()
                    .map(|record| {
                        url_to_source_package.remove(&record.url).map_or_else(
                            || PixiRecord::Binary(record),
                            |(source_record, _repodata_record)| {
                                PixiRecord::Source(source_record.clone())
                            },
                        )
                    })
                    .collect_vec(),
            )
        })
        .await;

        // Error out if the background task failed or was canceled.
        match solve_result.map_err(JoinError::try_into_panic) {
            Err(Err(_)) => Err(CommandDispatcherError::Cancelled),
            Err(Ok(panic)) => std::panic::resume_unwind(panic),
            Ok(Err(err)) => Err(CommandDispatcherError::Failed(err)),
            Ok(Ok(result)) => Ok(result),
        }
    }
}

/// Generates a unique URL for a source record.
fn unique_url(source: &SourceRecord) -> Url {
    let mut url = source.source.identifiable_url();

    // Add unique identifiers to the URL.
    url.query_pairs_mut()
        .append_pair("name", source.package_record.name.as_source())
        .append_pair("version", &source.package_record.version.as_str())
        .append_pair("build", &source.package_record.build)
        .append_pair("subdir", &source.package_record.subdir);

    url
}

#[derive(Debug, thiserror::Error)]
pub enum SolveCondaEnvironmentError {
    #[error(transparent)]
    SolveError(#[from] rattler_solve::SolveError),

    #[error(transparent)]
    SpecConversionError(#[from] pixi_spec::SpecConversionError),
}
