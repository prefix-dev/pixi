use std::{collections::HashMap, path::PathBuf, sync::Arc};

use chrono::{DateTime, Utc};
use itertools::Itertools;
use pixi_record::{PixiRecord, SourceRecord};
use pixi_spec::{BinarySpec, SourceSpec};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{
    ChannelConfig, ChannelUrl, GenericVirtualPackage, MatchSpec, Platform, RepoDataRecord, Version,
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

    /// Dev source records whose dependencies should be installed.
    #[serde(skip)]
    pub dev_source_records: Vec<pixi_record::DevSourceRecord>,

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
            dev_source_records: vec![],
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

            // Create match specs for dev source packages themselves
            // Use a special prefix to avoid name clashes with real packages
            // When multiple variants exist for the same package, we only create one match spec
            // and let the solver choose which variant to use based on the constraints.
            // TODO: It would be nicer if the rattler solver could handle this directly
            // by introducing a special type of name/package for these virtual dependencies
            // that represent "install my dependencies but not me" packages.
            let dev_source_match_specs: Vec<_> = self
                .dev_source_records
                .iter()
                .map(|dev_source| dev_source.name.clone())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .map(|name| {
                    let prefixed_name = format!("__pixi_dev_source_{}", name.as_normalized());
                    MatchSpec {
                        name: Some(rattler_conda_types::PackageName::new_unchecked(
                            prefixed_name,
                        )),
                        ..MatchSpec::default()
                    }
                })
                .collect();

            // Construct repodata records for source records and dev sources so that we can feed them to the
            // solver.
            let mut url_to_source_package = HashMap::new();
            let mut url_to_dev_source = HashMap::new();

            // Add source records
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

            // Collect all dev source names for filtering
            let dev_source_names: std::collections::HashSet<_> = self
                .dev_source_records
                .iter()
                .map(|ds| ds.name.clone())
                .collect();

            // Add dev source records
            for dev_source in &self.dev_source_records {
                let url = unique_dev_source_url(dev_source);
                let prefixed_name =
                    format!("__pixi_dev_source_{}", dev_source.name.as_normalized());
                let build_string = dev_source_build_string(dev_source);
                let repodata_record = RepoDataRecord {
                    package_record: rattler_conda_types::PackageRecord {
                        subdir: self.platform.to_string(),
                        depends: dev_source
                            .dependencies
                            .iter_specs()
                            .filter(|(name, _)| !dev_source_names.contains(*name))
                            .map(|(name, spec)| {
                                let nameless = spec
                                    .clone()
                                    .try_into_nameless_match_spec_ref(&self.channel_config)
                                    .unwrap_or_default();
                                MatchSpec::from_nameless(nameless, Some(name.clone())).to_string()
                            })
                            .collect(),
                        constrains: dev_source
                            .constraints
                            .iter_specs()
                            .filter(|(name, _)| !dev_source_names.contains(*name))
                            .filter_map(|(name, spec)| {
                                let nameless = spec
                                    .clone()
                                    .try_into_nameless_match_spec(&self.channel_config)
                                    .ok()?;
                                Some(
                                    MatchSpec::from_nameless(nameless, Some(name.clone()))
                                        .to_string(),
                                )
                            })
                            .collect(),
                        ..rattler_conda_types::PackageRecord::new(
                            rattler_conda_types::PackageName::new_unchecked(prefixed_name.clone()),
                            Version::major(0),
                            build_string.clone(),
                        )
                    },
                    url: url.clone(),
                    file_name: format!("{}-0-{}.devsource", prefixed_name, build_string),
                    channel: None,
                };
                url_to_dev_source.insert(url, (dev_source, repodata_record));
            }

            // Collect repodata records from the remote servers, source metadata, and dev sources
            // together. The source and dev source records go into the first "channel" to ensure
            // they are picked first.
            //
            // TODO: This only holds up when the channel priority is strict. We should
            // probably enforce this better somehow..
            let mut solvable_records = Vec::with_capacity(self.binary_repodata.len() + 1);
            solvable_records.push(
                url_to_source_package
                    .values()
                    .map(|(_, record)| record)
                    .chain(url_to_dev_source.values().map(|(_, record)| record))
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
                    .chain(dev_source_match_specs)
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
                    .filter_map(|record| {
                        if let Some(source_record) = url_to_source_package.remove(&record.url) {
                            // This is a source package, we want to return the source record
                            // instead of the binary record.
                            return Some(PixiRecord::Source(source_record.0.clone()));
                        } else if let Some(_dev_source) = url_to_dev_source.remove(&record.url) {
                            // This is a dev source, we don't want to return it.
                            return None;
                        }

                        Some(PixiRecord::Binary(record))
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

/// Generates a unique URL for a dev source record.
fn unique_dev_source_url(dev_source: &pixi_record::DevSourceRecord) -> Url {
    let mut url = dev_source.source.identifiable_url();

    // Add unique identifiers to the URL.
    let mut pairs = url.query_pairs_mut();
    pairs.append_pair("name", dev_source.name.as_source());

    for (key, value) in &dev_source.variants {
        pairs.append_pair(&format!("_{}", key), value);
    }

    drop(pairs);

    url
}

/// Generates a unique build string for a dev source record based on its variants.
/// Uses a hash of the variants to ensure uniqueness when multiple variants exist.
fn dev_source_build_string(dev_source: &pixi_record::DevSourceRecord) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    // Hash the variants to create a stable, unique build string
    let mut hasher = DefaultHasher::new();
    dev_source.variants.hash(&mut hasher);
    let hash = hasher.finish();
    format!("{:x}", hash)
}

#[derive(Debug, thiserror::Error)]
pub enum SolveCondaEnvironmentError {
    #[error(transparent)]
    SolveError(#[from] rattler_solve::SolveError),

    #[error(transparent)]
    SpecConversionError(#[from] pixi_spec::SpecConversionError),
}
