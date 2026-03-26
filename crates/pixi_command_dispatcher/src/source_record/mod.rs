use chrono::Utc;
use itertools::{Either, Itertools};
use miette::Diagnostic;
use pixi_build_types::procedures::conda_outputs::CondaOutput;
use pixi_record::{FullSourceRecordData, PixiRecord, SourceRecord};
use pixi_spec::{BinarySpec, PixiSpec, SourceAnchor, SourceLocationSpec, SpecConversionError};
use pixi_spec_containers::DependencyMap;
use pixi_variant::{VariantSelector, VariantValue};
use rattler_conda_types::{
    ChannelConfig, InvalidPackageNameError, MatchSpec, PackageName, PackageRecord,
    package::RunExportsJson,
};
use rattler_repodata_gateway::{RunExportExtractorError, RunExportsReporter};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use thiserror::Error;
use tracing::instrument;

use crate::cache::build_backend_metadata::BuildBackendMetadataCache;
use crate::cache::common::CacheRevision;
use crate::cache::source_record::CachedSourceRecord;
use crate::{
    BuildBackendMetadataError, BuildBackendMetadataSpec, BuildEnvironment, CommandDispatcher,
    CommandDispatcherError, CommandDispatcherErrorResultExt, PackageNotProvidedError,
    PixiEnvironmentSpec, SolvePixiEnvironmentError,
    build::{Dependencies, DependenciesError, PinnedSourceCodeLocation, PixiRunExports},
    cache::{
        common::{CacheEntry, CacheKey, MetadataCache},
        source_record::{
            self as source_record_cache, SourceRecordCache, SourceRecordCacheEntry,
            SourceRecordCacheKey,
        },
    },
    source_metadata::cycle::{Cycle, CycleEnvironment},
};

/// A request for the resolved metadata of a single source record, identified
/// by package name and variant combination.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceRecordSpec {
    /// The name of the package to retrieve metadata from.
    pub package: PackageName,

    /// The specific variant that identifies which build output to resolve.
    pub variants: BTreeMap<String, VariantValue>,

    /// Information about the build backend to request the information from.
    pub backend_metadata: BuildBackendMetadataSpec,

    /// The timestamp exclusion to apply when resolving dependencies.
    pub exclude_newer: Option<chrono::DateTime<chrono::Utc>>,
}

/// In-memory deduplication key for `SourceRecordSpec`. Excludes `exclude_newer`
/// so that requests with different timestamps are deduplicated.
#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize)]
pub struct SourceRecordDeduplicationKey {
    pub package: PackageName,
    pub variants: BTreeMap<String, VariantValue>,
    pub backend_metadata: BuildBackendMetadataSpec,
}

impl SourceRecordDeduplicationKey {
    pub fn new(spec: &SourceRecordSpec) -> Self {
        Self {
            package: spec.package.clone(),
            variants: spec.variants.clone(),
            backend_metadata: spec.backend_metadata.clone(),
        }
    }
}

/// The result of resolving a single source record.
#[derive(Debug)]
pub struct ResolvedSourceRecord {
    /// Manifest and optional build source location for this record.
    pub source: PinnedSourceCodeLocation,

    /// The resolved source record.
    pub record: SourceRecord,
}

impl SourceRecordSpec {
    #[instrument(
        skip_all,
        name = "source-record",
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
        reporter: Option<Arc<dyn RunExportsReporter>>,
    ) -> Result<ResolvedSourceRecord, CommandDispatcherError<SourceRecordError>> {
        // Get the metadata from the build backend.
        let build_backend_metadata = command_dispatcher
            .build_backend_metadata(self.backend_metadata.clone())
            .await
            .map_err_with(SourceRecordError::BuildBackendMetadata);

        let build_backend_metadata = build_backend_metadata?;

        tracing::trace!(
            "Resolving source record for package {} with variants {:?}",
            self.package.as_source(),
            self.variants,
        );

        // Get the skip_cache flag from the build backend metadata
        let skip_read_cache = build_backend_metadata.skip_cache;

        let cache_key: CacheKey<SourceRecordCache> = SourceRecordCacheKey {
            package: self.package.clone(),
            variants: self.variants.clone(),
            channel_urls: self.backend_metadata.channels.clone(),
            build_environment: self.backend_metadata.build_environment.clone(),
            enabled_protocols: self.backend_metadata.enabled_protocols.clone(),
            source: build_backend_metadata.source.clone().into(),
            exclude_newer: self.exclude_newer,
        };
        let cache_read_result = command_dispatcher
            .source_record_cache()
            .read(&cache_key)
            .await
            .map_err(SourceRecordError::Cache)
            .map_err(CommandDispatcherError::Failed)?;

        let (cached_metadata, cache_version) = match cache_read_result {
            Some((metadata, version)) => (Some(metadata), version),
            // Start at cache version 0 if no cache exists
            None => (None, 0),
        };

        if !skip_read_cache
            && let Some(cached_metadata) = Self::verify_cache_freshness(
                cached_metadata,
                &build_backend_metadata.metadata.revision,
            )
            .await?
        {
            tracing::debug!("Using cached source record");

            return Ok(ResolvedSourceRecord {
                source: build_backend_metadata.source.clone(),
                record: Self::amend_cached_source_record(
                    &build_backend_metadata.source,
                    cached_metadata.record,
                ),
            });
        }

        // Find the matching output by name + variants
        let selector = VariantSelector::new(self.variants.clone());
        let output = selector
            .find(
                build_backend_metadata
                    .metadata
                    .outputs
                    .iter()
                    .filter(|o| o.metadata.name == self.package),
                |o| &o.metadata.variant,
            )
            .ok_or_else(|| {
                let available_names = build_backend_metadata
                    .metadata
                    .outputs
                    .iter()
                    .map(|output| output.metadata.name.clone());
                CommandDispatcherError::Failed(
                    PackageNotProvidedError::new(
                        self.package.clone(),
                        build_backend_metadata.source.manifest_source().clone(),
                        available_names,
                    )
                    .into(),
                )
            })?;

        let record = self
            .resolve_output(
                &command_dispatcher,
                output,
                build_backend_metadata.source.clone(),
                reporter,
            )
            .await?;

        let cached_entry = SourceRecordCacheEntry {
            revision: CacheRevision::new(),
            cache_version,
            record: record.clone(),
            build_backend: (
                build_backend_metadata.cache_key.clone(),
                build_backend_metadata.metadata.revision.clone(),
            ),
        };

        // Try to store the metadata in the cache with version checking
        match command_dispatcher
            .source_record_cache()
            .try_write(&cache_key, cached_entry, cache_version)
            .await
            .map_err(SourceRecordError::Cache)
            .map_err(CommandDispatcherError::Failed)?
        {
            source_record_cache::WriteResult::Written => {
                tracing::trace!("Cache updated successfully");
            }
            source_record_cache::WriteResult::Conflict(_) => {
                tracing::warn!(
                    "Cache was updated by another process during computation (version conflict), using our computed result"
                );
            }
        }

        Ok(ResolvedSourceRecord {
            record: Self::amend_cached_source_record(&build_backend_metadata.source, record),
            source: build_backend_metadata.source.clone(),
        })
    }

    /// Converts a cached source record back into a full `SourceRecord` by
    /// adding the source location information that is derived from the cache key.
    fn amend_cached_source_record(
        source: &PinnedSourceCodeLocation,
        record: CachedSourceRecord,
    ) -> SourceRecord {
        let CachedSourceRecord {
            package_record,
            variants,
            sources,
            timestamp,
        } = record;
        SourceRecord {
            data: FullSourceRecordData {
                package_record,
                sources,
            },
            variants,
            timestamp,
            manifest_source: source.manifest_source().clone(),
            build_source: source.build_source().cloned(),
            identifier_hash: None,
        }
    }

    async fn verify_cache_freshness(
        cached_metadata: Option<CacheEntry<SourceRecordCache>>,
        current_build_backend_revision: &CacheRevision<BuildBackendMetadataCache>,
    ) -> Result<Option<CacheEntry<SourceRecordCache>>, CommandDispatcherError<SourceRecordError>>
    {
        let Some(cached_metadata) = cached_metadata else {
            tracing::debug!("no cached metadata passed.");
            return Ok(None);
        };

        if cached_metadata.build_backend.1 != *current_build_backend_revision {
            tracing::info!("Cached metadata is stale, skipping cache");
            return Ok(None);
        }

        Ok(Some(cached_metadata))
    }

    async fn resolve_output(
        &self,
        command_dispatcher: &CommandDispatcher,
        output: &CondaOutput,
        source: PinnedSourceCodeLocation,
        reporter: Option<Arc<dyn RunExportsReporter>>,
    ) -> Result<CachedSourceRecord, CommandDispatcherError<SourceRecordError>> {
        let manifest_source = source.manifest_source().clone();
        let source_anchor = SourceAnchor::from(SourceLocationSpec::from(manifest_source.clone()));

        // Create a common cut-off for the build and host environments.
        let exclude_newer = self.exclude_newer.unwrap_or_else(Utc::now);

        // Solve the build environment for the output.
        let mut compatibility_map = HashMap::new();
        let build_dependencies = output
            .build_dependencies
            .as_ref()
            .map(|deps| Dependencies::new(deps, Some(source_anchor.clone()), &compatibility_map))
            .transpose()
            .map_err(SourceRecordError::from)
            .map_err(CommandDispatcherError::Failed)?
            .unwrap_or_default();
        let mut build_records = self
            .solve_dependencies(
                self.package.clone(),
                CycleEnvironment::Build,
                command_dispatcher,
                build_dependencies.clone(),
                self.backend_metadata
                    .build_environment
                    .to_build_from_build(),
                exclude_newer,
            )
            .await?;

        let gateway = command_dispatcher.gateway();
        let build_run_exports = build_dependencies
            .extract_run_exports(
                &mut build_records,
                &output.ignore_run_exports,
                gateway,
                reporter.clone(),
            )
            .await
            .map_err(|err| {
                SourceRecordError::RunExportsExtraction(String::from("build"), Arc::new(err))
            })
            .map_err(CommandDispatcherError::Failed)?;

        compatibility_map.extend(
            build_records
                .iter()
                .map(|record| (record.package_record().name.clone(), record)),
        );

        // Solve the host environment for the output.
        let host_dependencies = output
            .host_dependencies
            .as_ref()
            .map(|deps| Dependencies::new(deps, Some(source_anchor.clone()), &compatibility_map))
            .transpose()
            .map_err(SourceRecordError::from)
            .map_err(CommandDispatcherError::Failed)?
            .unwrap_or_default()
            // Extend with the run exports from the build environment.
            .extend_with_run_exports_from_build(&build_run_exports);
        let mut host_records = self
            .solve_dependencies(
                self.package.clone(),
                CycleEnvironment::Host,
                command_dispatcher,
                host_dependencies.clone(),
                self.backend_metadata.build_environment.clone(),
                exclude_newer,
            )
            .await?;
        let host_run_exports = host_dependencies
            .extract_run_exports(
                &mut host_records,
                &output.ignore_run_exports,
                gateway,
                reporter,
            )
            .await
            .map_err(|err| {
                SourceRecordError::RunExportsExtraction(String::from("host"), Arc::new(err))
            })
            .map_err(CommandDispatcherError::Failed)?;

        compatibility_map.extend(
            host_records
                .iter()
                .map(|record| (record.package_record().name.clone(), record)),
        );

        // Gather the dependencies for the output.
        let run_dependencies =
            Dependencies::new(&output.run_dependencies, None, &compatibility_map)
                .map_err(SourceRecordError::from)
                .map_err(CommandDispatcherError::Failed)?
                .extend_with_run_exports_from_build_and_host(
                    host_run_exports,
                    build_run_exports,
                    output.metadata.subdir,
                );

        let PackageRecordDependencies {
            depends,
            constrains,
            mut sources,
        } = PackageRecordDependencies::new(run_dependencies, &self.backend_metadata.channel_config)
            .map_err(SourceRecordError::from)
            .map_err(CommandDispatcherError::Failed)?;

        // Convert the run exports
        let run_exports =
            PixiRunExports::try_from_protocol(&output.run_exports, &compatibility_map)
                .map_err(SourceRecordError::from)
                .map_err(CommandDispatcherError::Failed)?;

        let pixi_spec_to_match_spec = |name: &PackageName,
                                       spec: &PixiSpec,
                                       sources: &mut HashMap<PackageName, SourceLocationSpec>|
         -> Result<MatchSpec, SourceRecordError> {
            match spec.clone().into_source_or_binary() {
                Either::Left(source) => {
                    match sources.entry(name.clone()) {
                        std::collections::hash_map::Entry::Occupied(entry) => {
                            if entry.get() == &source.location {
                                return Err(SourceRecordError::DuplicateSourceDependency {
                                    package: name.clone(),
                                    source1: Box::new(entry.get().clone()),
                                    source2: Box::new(source.location.clone()),
                                });
                            }
                            entry.into_mut()
                        }
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            entry.insert(source.location.clone())
                        }
                    };
                    Ok(MatchSpec::from_nameless(
                        source.to_nameless_match_spec(),
                        name.clone().into(),
                    ))
                }
                Either::Right(binary) => {
                    let spec = binary
                        .try_into_nameless_match_spec(&self.backend_metadata.channel_config)
                        .map_err(SourceRecordError::from)?;
                    Ok(MatchSpec::from_nameless(spec, name.clone().into()))
                }
            }
        };

        let pixi_specs_to_match_spec = |specs: DependencyMap<PackageName, PixiSpec>,
                                        sources: &mut HashMap<PackageName, SourceLocationSpec>|
         -> Result<
            Vec<String>,
            CommandDispatcherError<SourceRecordError>,
        > {
            specs
                .into_specs()
                .map(|(name, spec)| Ok(pixi_spec_to_match_spec(&name, &spec, sources)?.to_string()))
                .collect::<Result<Vec<_>, SourceRecordError>>()
                .map_err(CommandDispatcherError::Failed)
        };

        let binary_specs_to_match_spec = |specs: DependencyMap<PackageName, BinarySpec>| -> Result<
            Vec<String>,
            CommandDispatcherError<SourceRecordError>,
        > {
            specs
                .into_specs()
                .map(|(name, spec)| {
                    let nameless_spec = spec
                        .try_into_nameless_match_spec(&self.backend_metadata.channel_config)
                        .map_err(SourceRecordError::from)?;
                    Ok(MatchSpec::from_nameless(nameless_spec, name.into()).to_string())
                })
                .collect::<Result<Vec<_>, SourceRecordError>>()
                .map_err(CommandDispatcherError::Failed)
        };

        // Gather the run exports for the output.
        let run_exports = RunExportsJson {
            weak: pixi_specs_to_match_spec(run_exports.weak, &mut sources)?,
            strong: pixi_specs_to_match_spec(run_exports.strong, &mut sources)?,
            noarch: pixi_specs_to_match_spec(run_exports.noarch, &mut sources)?,
            weak_constrains: binary_specs_to_match_spec(run_exports.weak_constrains)?,
            strong_constrains: binary_specs_to_match_spec(run_exports.strong_constrains)?,
        };

        // Compute the timestamp of the newest package that was used in the build/host environment.
        let newest_package_timestamp = host_records
            .iter()
            .chain(build_records.iter())
            .filter_map(|record| {
                record
                    .package_record()
                    .timestamp
                    .map(chrono::DateTime::<chrono::Utc>::from)
            })
            .max()
            .unwrap_or_else(chrono::Utc::now);

        Ok(CachedSourceRecord {
            package_record: PackageRecord {
                size: None,
                sha256: None,
                md5: None,
                timestamp: None,
                platform: output
                    .metadata
                    .subdir
                    .only_platform()
                    .map(ToString::to_string),
                arch: output
                    .metadata
                    .subdir
                    .arch()
                    .as_ref()
                    .map(ToString::to_string),
                name: output.metadata.name.clone(),
                build: output.metadata.build.clone(),
                version: output.metadata.version.clone(),
                build_number: output.metadata.build_number,
                license: output.metadata.license.clone(),
                subdir: output.metadata.subdir.to_string(),
                license_family: output.metadata.license_family.clone(),
                noarch: output.metadata.noarch,
                constrains,
                depends,
                run_exports: Some(run_exports),
                purls: output
                    .metadata
                    .purls
                    .as_ref()
                    .map(|purls| purls.iter().cloned().collect()),
                python_site_packages_path: output.metadata.python_site_packages_path.clone(),
                features: None,
                track_features: vec![],
                legacy_bz2_md5: None,
                legacy_bz2_size: None,
                experimental_extra_depends: Default::default(),
            },
            timestamp: newest_package_timestamp,
            sources: sources
                .into_iter()
                .map(|(name, source)| (name.as_source().to_string(), source))
                .collect(),
            variants: output
                .metadata
                .variant
                .iter()
                .map(|(k, v)| (k.clone(), VariantValue::from(v.clone())))
                .collect(),
        })
    }

    async fn solve_dependencies(
        &self,
        pkg_name: PackageName,
        env_type: CycleEnvironment,
        command_dispatcher: &CommandDispatcher,
        dependencies: Dependencies,
        build_environment: BuildEnvironment,
        exclude_newer: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<PixiRecord>, CommandDispatcherError<SourceRecordError>> {
        if dependencies.dependencies.is_empty() {
            return Ok(vec![]);
        }
        let preferred_build_source = self
            .backend_metadata
            .preferred_build_source
            .as_ref()
            .map(|pinned| BTreeMap::from([(pkg_name.clone(), pinned.clone())]))
            .unwrap_or_default();
        match command_dispatcher
            .solve_pixi_environment(PixiEnvironmentSpec {
                name: Some(format!("{} ({})", pkg_name.as_source(), env_type)),
                dependencies: dependencies
                    .dependencies
                    .into_specs()
                    .map(|(name, spec)| (name, spec.value))
                    .collect(),
                constraints: dependencies
                    .constraints
                    .into_specs()
                    .map(|(name, spec)| (name, spec.value))
                    .collect(),
                dev_sources: Default::default(),
                installed: vec![],
                build_environment,
                channels: self.backend_metadata.channels.clone(),
                strategy: Default::default(),
                channel_priority: Default::default(),
                exclude_newer: Some(exclude_newer),
                channel_config: self.backend_metadata.channel_config.clone(),
                variant_configuration: self.backend_metadata.variant_configuration.clone(),
                variant_files: self.backend_metadata.variant_files.clone(),
                enabled_protocols: self.backend_metadata.enabled_protocols.clone(),
                preferred_build_source,
            })
            .await
        {
            Err(CommandDispatcherError::Failed(SolvePixiEnvironmentError::Cycle(mut cycle))) => {
                cycle.stack.push((pkg_name, env_type));
                Err(CommandDispatcherError::Failed(SourceRecordError::Cycle(
                    cycle,
                )))
            }
            Err(CommandDispatcherError::Failed(e)) => match env_type {
                CycleEnvironment::Build => Err(CommandDispatcherError::Failed(
                    SourceRecordError::SolveBuildEnvironment(Box::new(e)),
                )),
                _ => Err(CommandDispatcherError::Failed(
                    SourceRecordError::SolveHostEnvironment(Box::new(e)),
                )),
            },
            Err(CommandDispatcherError::Cancelled) => Err(CommandDispatcherError::Cancelled),
            Ok(records) => Ok(records),
        }
    }
}

struct PackageRecordDependencies {
    pub depends: Vec<String>,
    pub constrains: Vec<String>,
    pub sources: HashMap<rattler_conda_types::PackageName, SourceLocationSpec>,
}

impl PackageRecordDependencies {
    pub fn new(
        dependencies: Dependencies,
        channel_config: &ChannelConfig,
    ) -> Result<PackageRecordDependencies, SpecConversionError> {
        let constrains = dependencies
            .constraints
            .into_specs()
            .map(|(name, spec)| {
                Ok(MatchSpec::from_nameless(
                    spec.value.try_into_nameless_match_spec(channel_config)?,
                    name.into(),
                ))
            })
            .map_ok(|spec| spec.to_string())
            .collect::<Result<Vec<_>, _>>()?;
        let mut depends = Vec::new();
        let mut sources = HashMap::new();
        for (name, spec) in dependencies.dependencies.into_specs() {
            match spec.value.into_source_or_binary() {
                Either::Left(source) => {
                    let spec = MatchSpec::from_nameless(
                        source.to_nameless_match_spec(),
                        name.clone().into(),
                    );
                    depends.push(spec.to_string());
                    sources.insert(name, source.location);
                }
                Either::Right(binary) => {
                    if let Ok(spec) = binary.try_into_nameless_match_spec(channel_config) {
                        depends.push(MatchSpec::from_nameless(spec, name.into()).to_string());
                    }
                }
            }
        }
        Ok(PackageRecordDependencies {
            depends,
            constrains,
            sources,
        })
    }
}

#[derive(Debug, Clone, Error, Diagnostic)]
pub enum SourceRecordError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    BuildBackendMetadata(#[from] BuildBackendMetadataError),

    #[error("failed to amend run exports for {0} environment")]
    RunExportsExtraction(String, #[source] Arc<RunExportExtractorError>),

    #[error("while trying to solve the build environment for the package")]
    SolveBuildEnvironment(
        #[diagnostic_source]
        #[source]
        Box<SolvePixiEnvironmentError>,
    ),

    #[error("while trying to solve the host environment for the package")]
    SolveHostEnvironment(
        #[diagnostic_source]
        #[source]
        Box<SolvePixiEnvironmentError>,
    ),

    #[error(transparent)]
    SpecConversionError(Arc<SpecConversionError>),

    #[error(transparent)]
    InvalidPackageName(Arc<InvalidPackageNameError>),

    #[error(transparent)]
    PinCompatibleError(#[from] crate::build::pin_compatible::PinCompatibleError),

    #[error("found two source dependencies for {} but for different sources ({source1} and {source2})", package.as_source()
    )]
    DuplicateSourceDependency {
        package: PackageName,
        source1: Box<SourceLocationSpec>,
        source2: Box<SourceLocationSpec>,
    },

    #[error("the dependencies of some packages in the environment form a cycle")]
    Cycle(Cycle),

    #[error(transparent)]
    Cache(#[from] source_record_cache::SourceRecordCacheError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    PackageNotProvided(#[from] PackageNotProvidedError),
}

impl From<SpecConversionError> for SourceRecordError {
    fn from(err: SpecConversionError) -> Self {
        Self::SpecConversionError(Arc::new(err))
    }
}

impl From<InvalidPackageNameError> for SourceRecordError {
    fn from(err: InvalidPackageNameError) -> Self {
        Self::InvalidPackageName(Arc::new(err))
    }
}

impl From<DependenciesError> for SourceRecordError {
    fn from(value: DependenciesError) -> Self {
        match value {
            DependenciesError::InvalidPackageName(error) => {
                SourceRecordError::InvalidPackageName(error)
            }
            DependenciesError::PinCompatibleError(error) => {
                SourceRecordError::PinCompatibleError(error)
            }
        }
    }
}
