use crate::{
    BuildBackendMetadataError, BuildBackendMetadataSpec, BuildEnvironment, CommandDispatcher,
    CommandDispatcherError, CommandDispatcherErrorResultExt, PixiEnvironmentSpec,
    SolvePixiEnvironmentError, build::source_metadata_cache::MetadataKind,
    executor::ExecutorFutures,
};
use futures::TryStreamExt;
use itertools::Either;
use miette::Diagnostic;
use pixi_build_frontend::types::{CondaPackageMetadata, SourcePackageSpecV1};
use pixi_build_types::procedures::conda_outputs::{
    CondaOutputDependencies, CondaOutputIgnoreRunExports, CondaOutputMetadata, CondaRunExports,
};
use pixi_build_types::{BinaryPackageSpecV1, NamedSpecV1, PackageSpecV1};
use pixi_record::{InputHash, PinnedSourceSpec, PixiRecord, SourceRecord};
use pixi_spec::{
    BinarySpec, DetailedSpec, PixiSpec, SourceAnchor, SourceSpec, SpecConversionError,
    UrlBinarySpec,
};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{
    ChannelConfig, InvalidPackageNameError, MatchSpec, NamedChannelOrUrl, NamelessMatchSpec,
    PackageName, PackageRecord, ParseStrictness, Platform, VersionSpec, package::RunExportsJson,
};
use std::collections::HashMap;
use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize)]
pub struct SourceMetadataSpec {
    /// The name of the package to retrieve metadata from.
    pub package: PackageName,

    /// Information about the build backend to request the information from.
    pub backend_metadata: BuildBackendMetadataSpec,
}

/// The result of building a particular source record.
#[derive(Debug, Clone)]
pub struct SourceMetadata {
    /// Information about the source checkout that was used to build the
    /// package.
    pub source: PinnedSourceSpec,

    /// All the source records for this particular package.
    pub records: Vec<SourceRecord>,
}

impl SourceMetadataSpec {
    pub(crate) async fn request(
        self,
        command_dispatcher: CommandDispatcher,
    ) -> Result<SourceMetadata, CommandDispatcherError<SourceMetadataError>> {
        // Get the metadata from the build backend.
        let build_backend_metadata = command_dispatcher
            .build_backend_metadata(self.backend_metadata.clone())
            .await
            .map_err_with(SourceMetadataError::BuildBackendMetadata)?;

        match &build_backend_metadata.metadata.metadata {
            MetadataKind::GetMetadata { packages } => {
                // Convert the metadata to source records.
                let records = source_metadata_to_records(
                    &build_backend_metadata.source,
                    packages,
                    &self.package,
                    &build_backend_metadata.metadata.input_hash,
                );

                Ok(SourceMetadata {
                    source: build_backend_metadata.source.clone(),
                    records,
                })
            }
            MetadataKind::Outputs { outputs } => {
                let mut futures = ExecutorFutures::new(command_dispatcher.executor());
                for output in outputs {
                    if output.identifier.name != self.package {
                        continue;
                    }
                    futures.push(self.resolve_output(
                        &command_dispatcher,
                        output,
                        build_backend_metadata.metadata.input_hash.clone(),
                        build_backend_metadata.source.clone(),
                    ));
                }

                Ok(SourceMetadata {
                    source: build_backend_metadata.source.clone(),
                    records: futures.try_collect().await?,
                })
            }
        }
    }

    async fn resolve_output(
        &self,
        command_dispatcher: &CommandDispatcher,
        output: &CondaOutputMetadata,
        input_hash: Option<InputHash>,
        source: PinnedSourceSpec,
    ) -> Result<SourceRecord, CommandDispatcherError<SourceMetadataError>> {
        let source_anchor = SourceAnchor::from(SourceSpec::from(source.clone()));

        // Solve the build environment for the output.
        let build_dependencies = output
            .build_dependencies
            .as_ref()
            .map(|deps| Dependencies::new(deps, source_anchor.clone()))
            .unwrap_or_default();
        let build_records = self
            .solve_dependencies(
                format!("{} (build)", self.package.as_source()),
                command_dispatcher,
                build_dependencies.clone(),
                self.backend_metadata
                    .build_environment
                    .to_build_from_build(),
            )
            .await
            .map_err_with(Box::new)
            .map_err_with(SourceMetadataError::SolveBuildEnvironment)?;
        let build_run_exports =
            build_dependencies.run_exports(&build_records, &output.ignore_run_exports);

        // Solve the host environment for the output.
        let host_dependencies = output
            .build_dependencies
            .as_ref()
            .map(|deps| Dependencies::new(deps, source_anchor.clone()))
            .unwrap_or_default()
            .with_host_run_exports(&build_run_exports);
        let host_records = self
            .solve_dependencies(
                format!("{} (host)", self.package.as_source()),
                command_dispatcher,
                build_dependencies.clone(),
                self.backend_metadata.build_environment.clone(),
            )
            .await
            .map_err_with(Box::new)
            .map_err_with(SourceMetadataError::SolveBuildEnvironment)?;
        let host_run_exports =
            host_dependencies.run_exports(&host_records, &output.ignore_run_exports);

        // Gather the dependencies for the output.
        let PackageRecordDependencies {
            depends,
            constrains,
            mut sources,
        } = Dependencies::new(&output.run_dependencies, source_anchor.clone())
            .with_run_run_exports(
                host_run_exports,
                build_run_exports,
                output.identifier.subdir,
            )
            .into_package_record_fields(&self.backend_metadata.channel_config)
            .map_err(SourceMetadataError::SpecConversionError)
            .map_err(CommandDispatcherError::Failed)?;

        // Convert the run exports
        let run_exports = PixiRunExports::try_from_protocol(&output.run_exports)
            .map_err(CommandDispatcherError::Failed)?;

        let pixi_spec_to_match_spec = |name: &PackageName,
                                       spec: &PixiSpec,
                                       sources: &mut HashMap<PackageName, SourceSpec>|
         -> Result<MatchSpec, SourceMetadataError> {
            match spec.clone().into_source_or_binary() {
                Either::Left(source) => {
                    let source = source_anchor.resolve(source);
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
                        Some(name.clone()),
                    ))
                }
                Either::Right(binary) => {
                    let spec = binary
                        .try_into_nameless_match_spec(&self.backend_metadata.channel_config)
                        .map_err(SourceMetadataError::SpecConversionError)?;
                    Ok(MatchSpec::from_nameless(spec, Some(name.clone())))
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
                    Ok(MatchSpec::from_nameless(nameless_spec, Some(name)).to_string())
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
                // timestamp here.
                timestamp: None,

                // These values are derived from the build backend values.
                platform: output
                    .identifier
                    .subdir
                    .only_platform()
                    .map(ToString::to_string),
                arch: output
                    .identifier
                    .subdir
                    .arch()
                    .as_ref()
                    .map(ToString::to_string),

                // These values are passed by the build backend
                name: output.identifier.name.clone(),
                build: output.identifier.build.clone(),
                version: output.identifier.version.clone(),
                build_number: output.identifier.build_number,
                license: output.identifier.license.clone(),
                subdir: output.identifier.subdir.to_string(),
                license_family: output.identifier.license_family.clone(),
                noarch: output.identifier.noarch,
                constrains,
                depends,
                run_exports: Some(run_exports),

                // These are deprecated and no longer used.
                features: None,
                track_features: vec![],
                legacy_bz2_md5: None,
                legacy_bz2_size: None,
                python_site_packages_path: None,

                // TODO(baszalmstra): Add support for these.
                purls: None,

                // These are not important at this point.
                extra_depends: Default::default(),
            },
            source,
            input_hash,
            sources: sources
                .into_iter()
                .map(|(name, source)| (name.as_source().to_string(), source))
                .collect(),
        })
    }

    async fn solve_dependencies(
        &self,
        name: String,
        command_dispatcher: &CommandDispatcher,
        dependencies: Dependencies,
        build_environment: BuildEnvironment,
    ) -> Result<Vec<PixiRecord>, CommandDispatcherError<SolvePixiEnvironmentError>> {
        if dependencies.dependencies.is_empty() {
            return Ok(vec![]);
        }
        command_dispatcher
            .solve_pixi_environment(PixiEnvironmentSpec {
                name: Some(name),
                dependencies: dependencies.dependencies,
                constraints: dependencies.constraints,
                installed: vec![], // TODO: To lock build environments, fill this.
                build_environment,
                channels: self.backend_metadata.channels.clone(),
                strategy: Default::default(),
                channel_priority: Default::default(),
                exclude_newer: None,
                channel_config: self.backend_metadata.channel_config.clone(),
                variants: self.backend_metadata.variants.clone(),
                enabled_protocols: self.backend_metadata.enabled_protocols.clone(),
            })
            .await
    }
}

#[derive(Clone, Default)]
struct Dependencies {
    pub dependencies: DependencyMap<rattler_conda_types::PackageName, PixiSpec>,
    pub constraints: DependencyMap<rattler_conda_types::PackageName, BinarySpec>,
}

struct PackageRecordDependencies {
    pub depends: Vec<String>,
    pub constrains: Vec<String>,
    pub sources: HashMap<PackageName, SourceSpec>,
}

impl Dependencies {
    pub fn new(output: &CondaOutputDependencies, source_anchor: SourceAnchor) -> Self {
        let mut dependencies = DependencyMap::default();
        let mut constraints = DependencyMap::default();

        for depend in &output.depends {
            let Some(name) = PackageName::from_str(&depend.name).ok() else {
                continue;
            };
            match from_package_spec_v1(depend.spec.clone()).into_source_or_binary() {
                Either::Left(source) => {
                    dependencies.insert(name, PixiSpec::from(source_anchor.resolve(source)));
                }
                Either::Right(binary) => {
                    dependencies.insert(name, PixiSpec::from(binary));
                }
            }
        }

        for constraint in &output.constraints {
            let Some(name) = PackageName::from_str(&constraint.name).ok() else {
                continue;
            };
            constraints.insert(name, from_pixi_binary_spec_v1(constraint.spec.clone()));
        }

        Self {
            dependencies,
            constraints,
        }
    }

    pub fn with_host_run_exports(mut self, build_run_exports: &PixiRunExports) -> Self {
        for (name, spec) in build_run_exports.strong.iter_specs() {
            self.dependencies.insert(name.clone(), spec.clone());
        }

        for (name, spec) in build_run_exports.strong_constrains.iter_specs() {
            self.constraints.insert(name.clone(), spec.clone());
        }

        self
    }

    pub fn with_run_run_exports(
        mut self,
        host_run_exports: PixiRunExports,
        build_run_exports: PixiRunExports,
        target_platform: Platform,
    ) -> Self {
        let add_dependencies = |this: &mut Self, specs: DependencyMap<PackageName, PixiSpec>| {
            for (name, spec) in specs.into_specs() {
                this.dependencies.insert(name, spec);
            }
        };

        let add_constraints = |this: &mut Self, specs: DependencyMap<PackageName, BinarySpec>| {
            for (name, spec) in specs.into_specs() {
                this.constraints.insert(name, spec);
            }
        };

        if target_platform == Platform::NoArch {
            add_dependencies(&mut self, host_run_exports.noarch);
        } else {
            add_dependencies(&mut self, build_run_exports.strong);
            add_dependencies(&mut self, host_run_exports.strong);
            add_dependencies(&mut self, host_run_exports.weak);
            add_constraints(&mut self, build_run_exports.strong_constrains);
            add_constraints(&mut self, host_run_exports.strong_constrains);
            add_constraints(&mut self, host_run_exports.weak_constrains);
        }

        self
    }

    /// Extract run exports from the solved environments.
    pub fn run_exports(
        &self,
        records: &[PixiRecord],
        ignore: &CondaOutputIgnoreRunExports,
    ) -> PixiRunExports {
        let mut filter_run_exports = PixiRunExports::default();

        fn filter_match_specs<T: From<BinarySpec>>(
            specs: &[String],
            ignore: &CondaOutputIgnoreRunExports,
        ) -> Vec<(PackageName, T)> {
            specs
                .iter()
                .filter_map(move |spec| {
                    let (Some(name), spec) = MatchSpec::from_str(spec, ParseStrictness::Lenient)
                        .ok()?
                        .into_nameless()
                    else {
                        return None;
                    };
                    if ignore.by_name.contains(&name) {
                        return None;
                    }

                    let binary_spec = match spec {
                        NamelessMatchSpec {
                            url: Some(url),
                            sha256,
                            md5,
                            ..
                        } => BinarySpec::Url(UrlBinarySpec { url, sha256, md5 }),
                        NamelessMatchSpec {
                            version,
                            build: None,
                            build_number: None,
                            file_name: None,
                            extras: None,
                            channel: None,
                            subdir: None,
                            namespace: None,
                            md5: None,
                            sha256: None,
                            url: _,
                            license: None,
                        } => BinarySpec::Version(version.unwrap_or(VersionSpec::Any)),
                        NamelessMatchSpec {
                            version,
                            build,
                            build_number,
                            file_name,
                            channel,
                            subdir,
                            md5,
                            sha256,
                            license,

                            // Caught in the above case
                            url: _,

                            // Explicitly ignored
                            namespace: _,
                            extras: _,
                        } => BinarySpec::DetailedVersion(Box::new(DetailedSpec {
                            version,
                            build,
                            build_number,
                            file_name,
                            channel: channel
                                .map(|c| NamedChannelOrUrl::Url(c.base_url.clone().into())),
                            subdir,
                            md5,
                            sha256,
                            license,
                        })),
                    };

                    Some((name, binary_spec.into()))
                })
                .collect()
        }

        for record in records {
            // Only record run exports for packages that are direct dependencies.
            if !self
                .dependencies
                .contains_key(&record.package_record().name)
            {
                continue;
            }

            // Filter based on whether we want to ignore run exports for a particular
            // package.
            if ignore.from_package.contains(&record.package_record().name) {
                continue;
            }

            // Make sure we have valid run exports.
            let Some(run_exports) = &record.package_record().run_exports else {
                unimplemented!("Extracting run exports from other places is not implemented yet");
            };

            filter_run_exports
                .noarch
                .extend(filter_match_specs(&run_exports.noarch, ignore));
            filter_run_exports
                .strong
                .extend(filter_match_specs(&run_exports.strong, ignore));
            filter_run_exports
                .strong_constrains
                .extend(filter_match_specs(&run_exports.strong_constrains, ignore));
            filter_run_exports
                .weak
                .extend(filter_match_specs(&run_exports.weak, ignore));
            filter_run_exports
                .weak_constrains
                .extend(filter_match_specs(&run_exports.weak_constrains, ignore));
        }

        filter_run_exports
    }

    pub fn into_package_record_fields(
        self,
        channel_config: &ChannelConfig,
    ) -> Result<PackageRecordDependencies, SpecConversionError> {
        let constraints = self
            .constraints
            .into_match_specs(channel_config)?
            .into_iter()
            .map(|spec| spec.to_string())
            .collect();
        let mut dependencies = Vec::new();
        let mut sources = HashMap::new();
        for (name, spec) in self.dependencies.into_specs() {
            match spec.into_source_or_binary() {
                Either::Left(source) => {
                    dependencies.push(
                        MatchSpec {
                            name: Some(name.clone()),
                            ..MatchSpec::default()
                        }
                        .to_string(),
                    );
                    sources.insert(name, source);
                }
                Either::Right(binary) => {
                    if let Ok(spec) = binary.try_into_nameless_match_spec(channel_config) {
                        dependencies.push(MatchSpec::from_nameless(spec, Some(name)).to_string());
                    }
                }
            }
        }
        Ok(PackageRecordDependencies {
            depends: dependencies,
            constrains: constraints,
            sources,
        })
    }
}

/// A variant of [`RunExportsJson`] but with pixi data types.
#[derive(Debug, Default, Clone)]
pub struct PixiRunExports {
    pub noarch: DependencyMap<PackageName, PixiSpec>,
    pub strong: DependencyMap<PackageName, PixiSpec>,
    pub weak: DependencyMap<PackageName, PixiSpec>,

    pub strong_constrains: DependencyMap<PackageName, BinarySpec>,
    pub weak_constrains: DependencyMap<PackageName, BinarySpec>,
}

impl PixiRunExports {
    pub fn try_from_protocol(output: &CondaRunExports) -> Result<Self, SourceMetadataError> {
        fn convert_package_spec(
            specs: &[NamedSpecV1<PackageSpecV1>],
        ) -> Result<DependencyMap<PackageName, PixiSpec>, SourceMetadataError> {
            specs
                .iter()
                .cloned()
                .map(|named_spec| {
                    let spec = from_package_spec_v1(named_spec.spec);
                    let name = PackageName::from_str(&named_spec.name).map_err(|err| {
                        SourceMetadataError::InvalidPackageName(named_spec.name.to_owned(), err)
                    })?;
                    Ok((name, spec))
                })
                .collect()
        }

        fn convert_binary_spec(
            specs: &[NamedSpecV1<BinaryPackageSpecV1>],
        ) -> Result<DependencyMap<PackageName, BinarySpec>, SourceMetadataError> {
            specs
                .iter()
                .cloned()
                .map(|named_spec| {
                    let spec = from_pixi_binary_spec_v1(named_spec.spec);
                    let name = PackageName::from_str(&named_spec.name).map_err(|err| {
                        SourceMetadataError::InvalidPackageName(named_spec.name.to_owned(), err)
                    })?;
                    Ok((name, spec))
                })
                .collect()
        }

        Ok(PixiRunExports {
            weak: convert_package_spec(&output.weak)?,
            strong: convert_package_spec(&output.strong)?,
            noarch: convert_package_spec(&output.noarch)?,
            weak_constrains: convert_binary_spec(&output.weak_constrains)?,
            strong_constrains: convert_binary_spec(&output.strong_constrains)?,
        })
    }
}

pub(crate) fn source_metadata_to_records(
    source: &PinnedSourceSpec,
    packages: &[CondaPackageMetadata],
    package: &PackageName,
    input_hash: &Option<InputHash>,
) -> Vec<SourceRecord> {
    // Convert the metadata to repodata
    let packages = packages
        .iter()
        .filter(|pkg| pkg.name == *package)
        .map(|p| {
            SourceRecord {
                input_hash: input_hash.clone(),
                source: source.clone(),
                sources: p
                    .sources
                    .iter()
                    .map(|(name, source)| (name.clone(), from_pixi_source_spec_v1(source.clone())))
                    .collect(),
                package_record: PackageRecord {
                    // We cannot now these values from the metadata because no actual package
                    // was built yet.
                    size: None,
                    sha256: None,
                    md5: None,

                    // TODO(baszalmstra): Decide if it makes sense to include the current
                    // timestamp here.
                    timestamp: None,

                    // These values are derived from the build backend values.
                    platform: p.subdir.only_platform().map(ToString::to_string),
                    arch: p.subdir.arch().as_ref().map(ToString::to_string),

                    // These values are passed by the build backend
                    name: p.name.clone(),
                    build: p.build.clone(),
                    version: p.version.clone(),
                    build_number: p.build_number,
                    license: p.license.clone(),
                    subdir: p.subdir.to_string(),
                    license_family: p.license_family.clone(),
                    noarch: p.noarch,
                    constrains: p.constraints.iter().map(|c| c.to_string()).collect(),
                    depends: p.depends.iter().map(|c| c.to_string()).collect(),

                    // These are deprecated and no longer used.
                    features: None,
                    track_features: vec![],
                    legacy_bz2_md5: None,
                    legacy_bz2_size: None,
                    python_site_packages_path: None,

                    // TODO(baszalmstra): Add support for these.
                    purls: None,

                    // These are not important at this point.
                    run_exports: None,
                    extra_depends: Default::default(),
                },
            }
        })
        .collect();
    packages
}

pub fn from_pixi_source_spec_v1(source: SourcePackageSpecV1) -> pixi_spec::SourceSpec {
    match source {
        SourcePackageSpecV1::Url(url) => pixi_spec::SourceSpec::Url(pixi_spec::UrlSourceSpec {
            url: url.url,
            md5: url.md5,
            sha256: url.sha256,
        }),
        SourcePackageSpecV1::Git(git) => pixi_spec::SourceSpec::Git(pixi_spec::GitSpec {
            git: git.git,
            rev: git.rev.map(|r| match r {
                pixi_build_frontend::types::GitReferenceV1::Branch(b) => {
                    pixi_spec::GitReference::Branch(b)
                }
                pixi_build_frontend::types::GitReferenceV1::Tag(t) => {
                    pixi_spec::GitReference::Tag(t)
                }
                pixi_build_frontend::types::GitReferenceV1::Rev(rev) => {
                    pixi_spec::GitReference::Rev(rev)
                }
                pixi_build_frontend::types::GitReferenceV1::DefaultBranch => {
                    pixi_spec::GitReference::DefaultBranch
                }
            }),
            subdirectory: git.subdirectory,
        }),
        SourcePackageSpecV1::Path(path) => pixi_spec::SourceSpec::Path(pixi_spec::PathSourceSpec {
            path: path.path.into(),
        }),
    }
}

pub fn from_pixi_binary_spec_v1(spec: BinaryPackageSpecV1) -> pixi_spec::BinarySpec {
    match spec {
        BinaryPackageSpecV1 {
            url: Some(url),
            sha256,
            md5,
            ..
        } => BinarySpec::Url(UrlBinarySpec { url, md5, sha256 }),
        BinaryPackageSpecV1 {
            version: Some(version),
            build: None,
            build_number: None,
            file_name: None,
            channel: None,
            subdir: None,
            md5: None,
            sha256: None,
            license: None,
            url: _,
        } => BinarySpec::Version(version),
        BinaryPackageSpecV1 {
            version,
            build,
            build_number,
            file_name,
            channel,
            subdir,
            md5,
            sha256,
            license,
            url: _,
        } => BinarySpec::DetailedVersion(Box::new(DetailedSpec {
            version,
            build,
            build_number,
            file_name,
            channel: channel.map(NamedChannelOrUrl::Url),
            subdir,
            license,
            md5,
            sha256,
        })),
    }
}

pub fn from_package_spec_v1(source: PackageSpecV1) -> pixi_spec::PixiSpec {
    match source {
        PackageSpecV1::Source(source) => from_pixi_source_spec_v1(source).into(),
        PackageSpecV1::Binary(binary) => from_pixi_binary_spec_v1(*binary).into(),
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum SourceMetadataError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    BuildBackendMetadata(#[from] BuildBackendMetadataError),

    #[error("failed to solve the build environment")]
    SolveBuildEnvironment(
        #[diagnostic_source]
        #[source]
        Box<SolvePixiEnvironmentError>,
    ),

    #[error("failed to solve the host environment")]
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
}
