mod cycle;

use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

pub use cycle::{Cycle, CycleEnvironment};
use futures::TryStreamExt;
use itertools::{Either, Itertools};
use miette::Diagnostic;
use pixi_build_types::procedures::conda_outputs::CondaOutput;
use pixi_record::{PixiRecord, SourceRecord};
use pixi_spec::{BinarySpec, PixiSpec, SourceAnchor, SourceSpec, SpecConversionError};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{
    ChannelConfig, InvalidPackageNameError, MatchSpec, PackageName, PackageRecord,
    package::RunExportsJson,
};
use rattler_repodata_gateway::{RunExportExtractorError, RunExportsReporter};
use thiserror::Error;
use tracing::instrument;

use crate::cache::build_backend_metadata::CachedCondaMetadataId;
use crate::cache::source_metadata::CachedSourceMetadataId;
use crate::{
    BuildBackendMetadataError, BuildBackendMetadataSpec, BuildEnvironment, CommandDispatcher,
    CommandDispatcherError, CommandDispatcherErrorResultExt, PackageNotProvidedError,
    PixiEnvironmentSpec, SolvePixiEnvironmentError,
    build::{Dependencies, DependenciesError, PixiRunExports, SourceCodeLocation},
    cache::{
        common::MetadataCache,
        source_metadata::{self, CachedSourceMetadata, SourceMetadataCacheShard},
    },
    executor::ExecutorFutures,
};

#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize)]
pub struct SourceMetadataSpec {
    /// The name of the package to retrieve metadata from.
    pub package: PackageName,

    /// Information about the build backend to request the information from.
    pub backend_metadata: BuildBackendMetadataSpec,
}

/// The result of building a particular source record.
#[derive(Debug)]
pub struct SourceMetadata {
    /// Manifest and optional build source location for this metadata.
    pub source: SourceCodeLocation,

    /// The metadata that was acquired from the build backend.
    pub cached_metadata: CachedSourceMetadata,
}

impl SourceMetadataSpec {
    #[instrument(
        skip_all,
        name = "source-metadata",
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
    ) -> Result<SourceMetadata, CommandDispatcherError<SourceMetadataError>> {
        // Get the metadata from the build backend.
        let build_backend_metadata = command_dispatcher
            .build_backend_metadata(self.backend_metadata.clone())
            .await
            .map_err_with(SourceMetadataError::BuildBackendMetadata);

        let build_backend_metadata = build_backend_metadata?;

        tracing::trace!(
            "Retrieving source metadata for package {}",
            self.package.as_source()
        );

        let cache_key = self.cache_key();

        // Get the skip_cache flag from the build backend metadata
        let skip_cache = build_backend_metadata.skip_cache;

        let cache_read_result = command_dispatcher
            .source_metadata_cache()
            .read(&cache_key)
            .await
            .map_err(SourceMetadataError::Cache)
            .map_err(CommandDispatcherError::Failed)?;

        let (cached_metadata, cache_version) = match cache_read_result {
            Some((metadata, version)) => (Some(metadata), version),
            // Start at cache version 0 if no cache exists
            None => (None, 0),
        };

        if !skip_cache
            && let Some(cached_metadata) =
                Self::verify_cache_freshness(cached_metadata, build_backend_metadata.metadata.id)
                    .await?
            {
                tracing::debug!("Using cached source metadata for package",);
                return Ok(SourceMetadata {
                    source: build_backend_metadata.source.clone(),
                    cached_metadata,
                });
            }

        let mut futures = ExecutorFutures::new(command_dispatcher.executor());
        let source_location = build_backend_metadata.source.clone();
        for output in &build_backend_metadata.metadata.outputs {
            if output.metadata.name != self.package {
                continue;
            }
            futures.push(self.resolve_output(
                &command_dispatcher,
                output,
                source_location.clone(),
                reporter.clone(),
            ));
        }

        let records: Vec<SourceRecord> = futures.try_collect().await?;

        // Ensure the source provides the requested package
        if records.is_empty() {
            let available_names = build_backend_metadata
                .metadata
                .outputs
                .iter()
                .map(|output| output.metadata.name.clone());
            return Err(CommandDispatcherError::Failed(
                PackageNotProvidedError::new(
                    self.package,
                    build_backend_metadata.source.manifest_source().clone(),
                    available_names,
                )
                .into(),
            ));
        }

        let cached_source_metadata = CachedSourceMetadata {
            id: CachedSourceMetadataId::random(),
            cache_version,
            records,
            cached_conda_metadata_id: build_backend_metadata.metadata.id,
        };

        // Try to store the metadata in the cache with version checking
        match command_dispatcher
            .source_metadata_cache()
            .try_write(&cache_key, cached_source_metadata.clone(), cache_version)
            .await
            .map_err(SourceMetadataError::Cache)
            .map_err(CommandDispatcherError::Failed)?
        {
            source_metadata::WriteResult::Written => {
                tracing::trace!("Cache updated successfully");
            }
            source_metadata::WriteResult::Conflict(_) => {
                tracing::warn!(
                    "Cache was updated by another process during computation (version conflict), using our computed result"
                );
            }
        }

        Ok(SourceMetadata {
            cached_metadata: cached_source_metadata,
            source: source_location,
        })
    }

    /// Computes the cache key for this instance
    pub(crate) fn cache_key(&self) -> SourceMetadataCacheShard {
        SourceMetadataCacheShard {
            package: self.package.clone(),
            channel_urls: self.backend_metadata.channels.clone(),
            build_environment: self.backend_metadata.build_environment.clone(),
            enabled_protocols: self.backend_metadata.enabled_protocols.clone(),
            pinned_source: self.backend_metadata.manifest_source.clone(),
        }
    }

    async fn verify_cache_freshness(
        cached_metadata: Option<CachedSourceMetadata>,
        current_conda_metadata_id: CachedCondaMetadataId,
    ) -> Result<Option<CachedSourceMetadata>, CommandDispatcherError<SourceMetadataError>> {
        let Some(cached_metadata) = cached_metadata else {
            tracing::debug!("no cached metadata passed.");
            return Ok(None);
        };

        if cached_metadata.cached_conda_metadata_id != current_conda_metadata_id {
            // Not adding tracing here because the backend metadata would already have done that.
            tracing::info!("Cached metadata is stale, skipping cache");
            return Ok(None);
        }

        Ok(Some(cached_metadata))
    }

    async fn resolve_output(
        &self,
        command_dispatcher: &CommandDispatcher,
        output: &CondaOutput,
        source: SourceCodeLocation,
        reporter: Option<Arc<dyn RunExportsReporter>>,
    ) -> Result<SourceRecord, CommandDispatcherError<SourceMetadataError>> {
        let manifest_source = source.manifest_source().clone();
        let build_source = source.build_source().cloned();
        let source_anchor = SourceAnchor::from(SourceSpec::from(manifest_source.clone()));

        // Solve the build environment for the output.
        let build_dependencies = output
            .build_dependencies
            .as_ref()
            // TODO(tim): we need to check if this works for out-of-tree builds with source dependencies in the out-of-tree, this might be incorrectly anchored
            .map(|deps| Dependencies::new(deps, Some(source_anchor.clone())))
            .transpose()
            .map_err(SourceMetadataError::from)
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
            .map_err(|err| SourceMetadataError::RunExportsExtraction(String::from("build"), err))
            .map_err(CommandDispatcherError::Failed)?;

        // Solve the host environment for the output.
        let host_dependencies = output
            .host_dependencies
            .as_ref()
            .map(|deps| Dependencies::new(deps, Some(source_anchor.clone())))
            .transpose()
            .map_err(SourceMetadataError::from)
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
            .map_err(|err| SourceMetadataError::RunExportsExtraction(String::from("host"), err))
            .map_err(CommandDispatcherError::Failed)?;

        // Gather the dependencies for the output.
        let run_dependencies = Dependencies::new(&output.run_dependencies, None)
            .map_err(SourceMetadataError::from)
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
            .map_err(SourceMetadataError::SpecConversionError)
            .map_err(CommandDispatcherError::Failed)?;

        // Convert the run exports
        let run_exports = PixiRunExports::try_from_protocol(&output.run_exports)
            .map_err(SourceMetadataError::from)
            .map_err(CommandDispatcherError::Failed)?;

        let pixi_spec_to_match_spec = |name: &PackageName,
                                       spec: &PixiSpec,
                                       sources: &mut HashMap<PackageName, SourceSpec>|
         -> Result<MatchSpec, SourceMetadataError> {
            match spec.clone().into_source_or_binary() {
                Either::Left(source) => {
                    let source = match sources.entry(name.clone()) {
                        std::collections::hash_map::Entry::Occupied(entry) => {
                            // If the entry already exists, check if it points to the same source.
                            if entry.get() == &source {
                                return Err(SourceMetadataError::DuplicateSourceDependency {
                                    package: name.clone(),
                                    source1: Box::new(entry.get().clone()),
                                    source2: Box::new(source.clone()),
                                });
                            }
                            entry.into_mut()
                        }
                        std::collections::hash_map::Entry::Vacant(entry) => entry.insert(source),
                    };
                    Ok(MatchSpec::from_nameless(
                        source.to_nameless_match_spec(),
                        Some(name.clone().into()),
                    ))
                }
                Either::Right(binary) => {
                    let spec = binary
                        .try_into_nameless_match_spec(&self.backend_metadata.channel_config)
                        .map_err(SourceMetadataError::SpecConversionError)?;
                    Ok(MatchSpec::from_nameless(spec, Some(name.clone().into())))
                }
            }
        };

        let pixi_specs_to_match_spec = |specs: DependencyMap<PackageName, PixiSpec>,
                                        sources: &mut HashMap<PackageName, SourceSpec>|
         -> Result<
            Vec<String>,
            CommandDispatcherError<SourceMetadataError>,
        > {
            specs
                .into_specs()
                .map(|(name, spec)| Ok(pixi_spec_to_match_spec(&name, &spec, sources)?.to_string()))
                .collect::<Result<Vec<_>, SourceMetadataError>>()
                .map_err(CommandDispatcherError::Failed)
        };

        let binary_specs_to_match_spec = |specs: DependencyMap<PackageName, BinarySpec>| -> Result<
            Vec<String>,
            CommandDispatcherError<SourceMetadataError>,
        > {
            specs
                .into_specs()
                .map(|(name, spec)| {
                    let nameless_spec = spec
                        .try_into_nameless_match_spec(&self.backend_metadata.channel_config)
                        .map_err(SourceMetadataError::SpecConversionError)?;
                    Ok(MatchSpec::from_nameless(nameless_spec, Some(name.into())).to_string())
                })
                .collect::<Result<Vec<_>, SourceMetadataError>>()
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

        Ok(SourceRecord {
            package_record: PackageRecord {
                // We cannot now these values from the metadata because no actual package
                // was built yet.
                size: None,
                sha256: None,
                md5: None,

                // TODO(baszalmstra): Decide if it makes sense to include the current
                //  timestamp here.
                timestamp: None,

                // These values are derived from the build backend values.
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

                // These values are passed by the build backend
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

                // These are deprecated and no longer used.
                features: None,
                track_features: vec![],
                legacy_bz2_md5: None,
                legacy_bz2_size: None,

                // These are not important at this point.
                experimental_extra_depends: Default::default(),
            },
            manifest_source,
            build_source,
            sources: sources
                .into_iter()
                .map(|(name, source)| (name.as_source().to_string(), source))
                .collect(),
            variants: Some(
                output
                    .metadata
                    .variant
                    .iter()
                    .map(|(k, v)| (k.clone(), pixi_record::VariantValue::from(v.clone())))
                    .collect(),
            ),
        })
    }

    async fn solve_dependencies(
        &self,
        pkg_name: PackageName,
        env_type: CycleEnvironment,
        command_dispatcher: &CommandDispatcher,
        dependencies: Dependencies,
        build_environment: BuildEnvironment,
    ) -> Result<Vec<PixiRecord>, CommandDispatcherError<SourceMetadataError>> {
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
                installed: vec![], // TODO: To lock build environments, fill this.
                build_environment,
                channels: self.backend_metadata.channels.clone(),
                strategy: Default::default(),
                channel_priority: Default::default(),
                exclude_newer: None,
                channel_config: self.backend_metadata.channel_config.clone(),
                variant_configuration: self.backend_metadata.variant_configuration.clone(),
                variant_files: self.backend_metadata.variant_files.clone(),
                enabled_protocols: self.backend_metadata.enabled_protocols.clone(),
                preferred_build_source,
            })
            .await
        {
            Err(CommandDispatcherError::Failed(SolvePixiEnvironmentError::Cycle(mut cycle))) => {
                // If a cycle was detected, add the current environment to the cycle.
                cycle.stack.push((pkg_name, env_type));
                Err(CommandDispatcherError::Failed(SourceMetadataError::Cycle(
                    cycle,
                )))
            }
            Err(CommandDispatcherError::Failed(e)) => {
                // If solving failed, we return an error based on the environment type that we
                // tried to solve.
                match env_type {
                    CycleEnvironment::Build => Err(CommandDispatcherError::Failed(
                        SourceMetadataError::SolveBuildEnvironment(Box::new(e)),
                    )),
                    _ => Err(CommandDispatcherError::Failed(
                        SourceMetadataError::SolveHostEnvironment(Box::new(e)),
                    )),
                }
            }
            Err(CommandDispatcherError::Cancelled) => Err(CommandDispatcherError::Cancelled),
            Ok(records) => Ok(records),
        }
    }
}

struct PackageRecordDependencies {
    pub depends: Vec<String>,
    pub constrains: Vec<String>,
    pub sources: HashMap<rattler_conda_types::PackageName, SourceSpec>,
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
                    Some(name.into()),
                ))
            })
            .map_ok(|spec| spec.to_string())
            .collect::<Result<Vec<_>, _>>()?;
        let mut depends = Vec::new();
        let mut sources = HashMap::new();
        for (name, spec) in dependencies.dependencies.into_specs() {
            match spec.value.into_source_or_binary() {
                Either::Left(source) => {
                    depends.push(
                        MatchSpec {
                            name: Some(name.clone().into()),
                            ..MatchSpec::default()
                        }
                        .to_string(),
                    );
                    sources.insert(name, source);
                }
                Either::Right(binary) => {
                    if let Ok(spec) = binary.try_into_nameless_match_spec(channel_config) {
                        depends.push(MatchSpec::from_nameless(spec, Some(name.into())).to_string());
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

#[derive(Debug, Error, Diagnostic)]
pub enum SourceMetadataError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    BuildBackendMetadata(#[from] BuildBackendMetadataError),

    #[error("failed to amend run exports for {0} environment")]
    RunExportsExtraction(String, #[source] RunExportExtractorError),

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
    SpecConversionError(#[from] SpecConversionError),

    #[error("backend returned a dependency on an invalid package name: {0}")]
    InvalidPackageName(String, #[source] InvalidPackageNameError),

    #[error("found two source dependencies for {} but for different sources ({source1} and {source2})", package.as_source()
    )]
    DuplicateSourceDependency {
        package: PackageName,
        source1: Box<SourceSpec>,
        source2: Box<SourceSpec>,
    },

    #[error("the dependencies of some packages in the environment form a cycle")]
    Cycle(Cycle),

    #[error(transparent)]
    Cache(#[from] source_metadata::SourceMetadataCacheError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    PackageNotProvided(#[from] PackageNotProvidedError),
}

impl From<DependenciesError> for SourceMetadataError {
    fn from(value: DependenciesError) -> Self {
        match value {
            DependenciesError::InvalidPackageName(name, error) => {
                SourceMetadataError::InvalidPackageName(name, error)
            }
        }
    }
}
