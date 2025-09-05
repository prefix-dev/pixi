use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use miette::Diagnostic;
use pixi_build_discovery::EnabledProtocols;
use pixi_build_frontend::Backend;
use pixi_build_types::procedures::conda_outputs::CondaOutputsParams;
use pixi_record::{PinnedSourceSpec, PixiRecord};
use pixi_spec::{SourceAnchor, SourceSpec};
use rattler_conda_types::{
    ChannelConfig, ChannelUrl, ConvertSubdirError, InvalidPackageNameError, PackageRecord,
    Platform, RepoDataRecord, prefix::Prefix,
};
use rattler_digest::Sha256Hash;
use rattler_repodata_gateway::{RunExportExtractorError, RunExportsReporter};
use serde::Serialize;
use thiserror::Error;
use tracing::instrument;
use url::Url;

use crate::{
    BackendSourceBuildError, BackendSourceBuildMethod, BackendSourceBuildPrefix,
    BackendSourceBuildSpec, BackendSourceBuildV0Method, BackendSourceBuildV1Method,
    BuildEnvironment, BuildProfile, CachedBuildStatus, CommandDispatcher, CommandDispatcherError,
    CommandDispatcherErrorResultExt, InstallPixiEnvironmentError, InstallPixiEnvironmentResult,
    InstallPixiEnvironmentSpec, InstantiateBackendError, InstantiateBackendSpec,
    PixiEnvironmentSpec, SolvePixiEnvironmentError, SourceBuildCacheStatusError,
    SourceBuildCacheStatusSpec, SourceCheckoutError,
    build::{
        BuildCacheError, BuildHostEnvironment, BuildHostPackage, CachedBuild,
        CachedBuildSourceInfo, Dependencies, DependenciesError, MoveError, PackageBuildInputHash,
        PixiRunExports, SourceRecordOrCheckout, WorkDirKey, move_file,
    },
    package_identifier::PackageIdentifier,
};

/// Describes all parameters required to build a conda package from a pixi
/// source package.
///
/// This task prepares the build environment for a source build and then
/// delegates the actual build to the backend through the
/// [`BackendSourceBuildSpec`]. This allows preparation (installing host, build,
/// envs) to progress concurrently while the actual building of the package can
/// be done serially.
#[derive(Debug, Clone, Serialize, Eq, PartialEq, Hash)]
pub struct SourceBuildSpec {
    /// The source to build
    pub package: PackageIdentifier,

    /// The location of the source code to build.
    pub source: PinnedSourceSpec,

    /// The channel configuration to use when resolving metadata
    pub channel_config: ChannelConfig,

    /// The channels to use for solving.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<ChannelUrl>,

    /// Information about host platform on which the package is build. Note that
    /// a package might be targeting noarch in which case the host platform
    /// should be used.
    ///
    /// If this field is omitted the build backend will use the current
    /// platform.
    pub build_environment: BuildEnvironment,

    /// The build profile to use for the build.
    pub build_profile: BuildProfile,

    /// Variant configuration
    pub variants: Option<BTreeMap<String, Vec<String>>>,

    /// The directory where to place the built package.
    pub output_directory: Option<PathBuf>,

    /// The working directory to use for the build. If this is `None` a
    /// deterministic workspace local directory will be used.
    pub work_directory: Option<PathBuf>,

    /// Whether the build directory should be cleared before building.
    pub clean: bool,

    /// The protocols that are enabled for this source
    #[serde(skip_serializing_if = "crate::is_default")]
    pub enabled_protocols: EnabledProtocols,
}

#[derive(Debug, Clone)]
pub struct SourceBuildResult {
    /// The location on disk where the built package is located.
    pub output_file: PathBuf,

    /// The repodata record associated with the built package.
    pub record: RepoDataRecord,
}

#[derive(Debug, Serialize)]
pub struct BuiltPackage {
    /// The location on disk where the built package is located.
    #[serde(skip)]
    pub output_file: PathBuf,

    /// The metadata of the built package.
    pub metadata: CachedBuildSourceInfo,
}

impl SourceBuildSpec {
    #[instrument(
        skip_all,
        name = "source-build",
        fields(
            source= %self.source,
            package = %self.package,
        )
    )]
    pub(crate) async fn build(
        mut self,
        command_dispatcher: CommandDispatcher,
        reporter: Option<Arc<dyn RunExportsReporter>>,
    ) -> Result<SourceBuildResult, CommandDispatcherError<SourceBuildError>> {
        // If the output directory is not set, we want to use the build cache. Read the
        // build cache in that case.
        let (output_directory, build_cache) =
            if let Some(output_directory) = self.output_directory.clone() {
                (output_directory, None)
            } else {
                // Query the source build cache.
                let build_cache = command_dispatcher
                    .source_build_cache_status(SourceBuildCacheStatusSpec {
                        package: self.package.clone(),
                        build_environment: self.build_environment.clone(),
                        source: self.source.clone(),
                        channels: self.channels.clone(),
                        channel_config: self.channel_config.clone(),
                        enabled_protocols: self.enabled_protocols.clone(),
                    })
                    .await
                    .map_err_with(SourceBuildError::from)?;

                if let CachedBuildStatus::UpToDate(cached_build) = &build_cache.cached_build {
                    // If the build is up to date, we can return the cached build.
                    return Ok(SourceBuildResult {
                        output_file: build_cache.cache_dir.join(&cached_build.record.file_name),
                        record: cached_build.record.clone(),
                    });
                }

                (build_cache.cache_dir.clone(), Some(build_cache))
            };

        // Check out the source code.
        let source_checkout = command_dispatcher
            .checkout_pinned_source(self.source.clone())
            .await
            .map_err_with(SourceBuildError::SourceCheckout)?;

        // Discover information about the build backend from the source code (cached by
        // path).
        let discovered_backend = command_dispatcher
            .discover_backend(
                &source_checkout.path,
                self.channel_config.clone(),
                self.enabled_protocols.clone(),
            )
            .await
            .map_err_with(SourceBuildError::Discovery)?;

        // Compute the package input hash for caching purposes.
        let package_build_input_hash = PackageBuildInputHash::from(discovered_backend.as_ref());

        // Instantiate the backend with the discovered information.
        let backend = command_dispatcher
            .instantiate_backend(InstantiateBackendSpec {
                backend_spec: discovered_backend
                    .backend_spec
                    .clone()
                    .resolve(SourceAnchor::from(SourceSpec::from(self.source.clone()))),
                init_params: discovered_backend.init_params.clone(),
                channel_config: self.channel_config.clone(),
                enabled_protocols: self.enabled_protocols.clone(),
            })
            .await
            .map_err_with(SourceBuildError::Initialize)?;

        // Determine the working directory for the build.
        let work_directory = match std::mem::take(&mut self.work_directory) {
            Some(work_directory) => work_directory,
            None => command_dispatcher.cache_dirs().working_dirs().join(
                WorkDirKey {
                    source: SourceRecordOrCheckout::Record {
                        pinned: self.source.clone(),
                        package_name: self.package.name.clone(),
                    },
                    host_platform: self.build_environment.host_platform,
                    build_backend: backend.identifier().to_string(),
                }
                .key(),
            ),
        };

        // Clean the working directory if requested.
        if self.clean {
            if let Err(err) = fs_err::remove_dir_all(&work_directory) {
                return Err(CommandDispatcherError::Failed(
                    SourceBuildError::CleanWorkingDirectory(work_directory, err),
                ));
            }
        }

        // Build the package based on the support backend capabilities.
        let mut built_source = if backend.capabilities().provides_conda_build_v1() {
            self.build_v1(
                command_dispatcher,
                backend,
                work_directory,
                package_build_input_hash,
                reporter,
            )
            .await?
        } else {
            self.build_v0(
                command_dispatcher,
                backend,
                work_directory,
                package_build_input_hash,
            )
            .await?
        };

        // Create the output directory if it does not exist.
        fs_err::create_dir_all(&output_directory).map_err(|err| {
            CommandDispatcherError::Failed(SourceBuildError::CreateOutputDirectory(err))
        })?;

        // At this point, the directory should exist, so we can canonicalize the path.
        let output_directory = fs_err::canonicalize(&output_directory)
            .map_err(CommandDispatcherError::Failed)
            .map_err_with(SourceBuildError::CreateOutputDirectory)?;

        // The output file should also exist.
        let output_file = match fs_err::canonicalize(&built_source.output_file) {
            Ok(output_file) => output_file,
            Err(_err) => {
                return Err(CommandDispatcherError::Failed(
                    SourceBuildError::MissingOutputFile(built_source.output_file),
                ));
            }
        };

        if output_file.parent() != Some(&output_directory) {
            // Take the file name of the file and move it to the output directory.
            let file_name = built_source
                .output_file
                .file_name()
                .expect("the build backend did not return a file name");
            let destination = output_directory.join(file_name);
            if let Err(err) = move_file(&output_file, &destination) {
                return Err(CommandDispatcherError::Failed(SourceBuildError::Move(
                    output_file,
                    output_directory,
                    err,
                )));
            }
            built_source.output_file = destination;
        }

        // TODO: Instead of reading this from the resulting file, maybe we can construct
        // this during the build?
        let output_file = built_source.output_file.clone();
        let read_index_json_fut = simple_spawn_blocking::tokio::run_blocking_task(move || {
            rattler_package_streaming::seek::read_package_file(&output_file)
                .map_err(|err| CommandDispatcherError::Failed(SourceBuildError::ReadIndexJson(err)))
        });

        // Read the SHA256 hash of the package file.
        let read_sha256_fut = compute_package_sha256(&built_source.output_file);

        // Wait for both futures to complete.
        let (sha, index_json) = tokio::try_join!(read_sha256_fut, read_index_json_fut)?;

        // Construct the record from the index JSON and the SHA256 hash.
        let record = RepoDataRecord {
            package_record: PackageRecord::from_index_json(index_json, None, Some(sha), None)
                .map_err(|err| {
                    CommandDispatcherError::Failed(SourceBuildError::ConvertSubdir(err))
                })?,
            file_name: built_source
                .output_file
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            url: Url::from_file_path(&built_source.output_file)
                .expect("the output file should be a valid URL"),
            channel: None,
        };

        // Update the cache entry if we have one.
        if let Some(build_cache) = build_cache {
            let mut entry = build_cache.entry.lock().await;
            entry
                .insert(CachedBuild {
                    source: source_checkout
                        .pinned
                        .is_mutable()
                        .then_some(built_source.metadata),
                    record: record.clone(),
                })
                .await
                .map_err(SourceBuildError::BuildCache)
                .map_err(CommandDispatcherError::Failed)?;
        }

        Ok(SourceBuildResult {
            output_file: built_source.output_file,
            record,
        })
    }

    /// Returns whether the package should be built in an editable mode.
    fn editable(&self) -> bool {
        self.build_profile == BuildProfile::Development && self.source.is_mutable()
    }

    async fn build_v0(
        self,
        command_dispatcher: CommandDispatcher,
        backend: Backend,
        work_directory: PathBuf,
        package_build_input_hash: PackageBuildInputHash,
    ) -> Result<BuiltPackage, CommandDispatcherError<SourceBuildError>> {
        let result = command_dispatcher
            .backend_source_build(BackendSourceBuildSpec {
                method: BackendSourceBuildMethod::BuildV0(BackendSourceBuildV0Method {
                    editable: self.editable(),
                    build_environment: self.build_environment,
                    variants: self.variants,
                    output_directory: self.output_directory,
                }),
                backend,
                package: self.package,
                source: self.source,
                work_directory,
                channels: self.channels,
                channel_config: self.channel_config,
            })
            .await
            .map_err_with(SourceBuildError::from)?;

        Ok(BuiltPackage {
            output_file: result.output_file,
            metadata: CachedBuildSourceInfo {
                globs: result.input_globs,
                build: Default::default(),
                host: Default::default(),
                package_build_input_hash: Some(package_build_input_hash),
            },
        })
    }

    async fn build_v1(
        self,
        command_dispatcher: CommandDispatcher,
        backend: Backend,
        work_directory: PathBuf,
        package_build_input_hash: PackageBuildInputHash,
        reporter: Option<Arc<dyn RunExportsReporter>>,
    ) -> Result<BuiltPackage, CommandDispatcherError<SourceBuildError>> {
        let source_anchor = SourceAnchor::from(SourceSpec::from(self.source.clone()));
        let host_platform = self.build_environment.host_platform;
        let build_platform = self.build_environment.build_platform;

        // Request the metadata from the backend.
        // TODO: Can we somehow cache this metadata?
        let outputs = backend
            .conda_outputs(CondaOutputsParams {
                host_platform,
                build_platform,
                variant_configuration: self.variants.clone(),
                work_directory: work_directory.clone(),
                channels: self.channels.clone(),
            })
            .await
            .map_err(BackendSourceBuildError::BuildError)
            .map_err(SourceBuildError::from)
            .map_err(CommandDispatcherError::Failed)?;

        // Find the output that we want to build.
        let output = outputs
            .outputs
            .into_iter()
            .find(|output| {
                output.metadata.name == self.package.name
                    && output.metadata.version == self.package.version
                    && output.metadata.build == self.package.build
                    && output.metadata.subdir.as_str() == self.package.subdir
            })
            .ok_or_else(|| {
                CommandDispatcherError::Failed(SourceBuildError::MissingOutput {
                    subdir: self.package.subdir.clone(),
                    name: self.package.name.as_normalized().to_string(),
                    version: self.package.version.to_string(),
                    build: self.package.build.clone(),
                })
            })?;

        // Determine final directories for everything.
        let directories = Directories::new(&work_directory, host_platform);

        // Solve the build environment.
        let build_dependencies = output
            .build_dependencies
            .as_ref()
            .map(|deps| Dependencies::new(deps, Some(source_anchor.clone())))
            .transpose()
            .map_err(SourceBuildError::from)
            .map_err(CommandDispatcherError::Failed)?
            .unwrap_or_default();
        let mut build_records = self
            .solve_dependencies(
                format!("{} (build)", self.package.name.as_source()),
                &command_dispatcher,
                build_dependencies.clone(),
                self.build_environment.to_build_from_build(),
            )
            .await
            .map_err_with(Box::new)
            .map_err_with(SourceBuildError::SolveBuildEnvironment)?;

        let gateway = command_dispatcher.gateway();
        let build_run_exports = build_dependencies
            .extract_run_exports(
                &mut build_records,
                &output.ignore_run_exports,
                gateway,
                reporter.clone(),
            )
            .await
            .map_err(SourceBuildError::from)
            .map_err(CommandDispatcherError::Failed)?;

        // Solve the host environment for the output.
        let host_dependencies = output
            .host_dependencies
            .as_ref()
            .map(|deps| Dependencies::new(deps, Some(source_anchor.clone())))
            .transpose()
            .map_err(SourceBuildError::from)
            .map_err(CommandDispatcherError::Failed)?
            .unwrap_or_default()
            // Extend with the run exports from the build environment.
            .extend_with_run_exports_from_build(&build_run_exports);
        let mut host_records = self
            .solve_dependencies(
                format!("{} (host)", self.package.name.as_source()),
                &command_dispatcher,
                host_dependencies.clone(),
                self.build_environment.clone(),
            )
            .await
            .map_err_with(Box::new)
            .map_err_with(SourceBuildError::SolveBuildEnvironment)?;
        let host_run_exports = host_dependencies
            .extract_run_exports(
                &mut host_records,
                &output.ignore_run_exports,
                command_dispatcher.gateway(),
                reporter,
            )
            .await
            .map_err(SourceBuildError::from)
            .map_err(CommandDispatcherError::Failed)?;

        // Install the build environment
        let build_prefix = if build_records.is_empty() {
            None
        } else {
            Some(
                command_dispatcher
                    .install_pixi_environment(InstallPixiEnvironmentSpec {
                        name: format!("{} (build)", self.package.name.as_source()),
                        records: build_records.clone(),
                        prefix: Prefix::create(&directories.build_prefix)
                            .map_err(SourceBuildError::CreateBuildEnvironmentDirectory)
                            .map_err(CommandDispatcherError::Failed)?,
                        installed: None,
                        ignore_packages: None,
                        build_environment: self.build_environment.to_build_from_build(),
                        force_reinstall: Default::default(),
                        channels: self.channels.clone(),
                        channel_config: self.channel_config.clone(),
                        variants: self.variants.clone(),
                        enabled_protocols: self.enabled_protocols.clone(),
                    })
                    .await
                    .map_err_with(Box::new)
                    .map_err_with(SourceBuildError::InstallBuildEnvironment)?,
            )
        };

        // Install the host environment.
        let host_prefix = if host_records.is_empty() {
            None
        } else {
            Some(
                command_dispatcher
                    .install_pixi_environment(InstallPixiEnvironmentSpec {
                        name: format!("{} (host)", self.package.name.as_source()),
                        records: host_records.clone(),
                        prefix: Prefix::create(&directories.host_prefix)
                            .map_err(SourceBuildError::CreateBuildEnvironmentDirectory)
                            .map_err(CommandDispatcherError::Failed)?,
                        installed: None,
                        ignore_packages: None,
                        build_environment: self.build_environment.to_build_from_build(),
                        force_reinstall: Default::default(),
                        channels: self.channels.clone(),
                        channel_config: self.channel_config.clone(),
                        variants: self.variants.clone(),
                        enabled_protocols: self.enabled_protocols.clone(),
                    })
                    .await
                    .map_err_with(Box::new)
                    .map_err_with(SourceBuildError::InstallBuildEnvironment)?,
            )
        };

        // Ensure the work directory exists.
        fs_err::create_dir_all(&work_directory).map_err(|err| {
            CommandDispatcherError::Failed(SourceBuildError::CreateWorkDirectory(err))
        })?;

        // Gather the dependencies for the output.
        let dependencies = Dependencies::new(&output.run_dependencies, None)
            .map_err(SourceBuildError::from)
            .map_err(CommandDispatcherError::Failed)?
            .extend_with_run_exports_from_build_and_host(
                host_run_exports,
                build_run_exports,
                output.metadata.subdir,
            );

        // Convert the run exports
        let run_exports = PixiRunExports::try_from_protocol(&output.run_exports)
            .map_err(SourceBuildError::from)
            .map_err(CommandDispatcherError::Failed)?;

        // Extract the repodata records from the build and host environments.
        let build_records = Self::extract_prefix_repodata(build_records, build_prefix);
        let host_records = Self::extract_prefix_repodata(host_records, host_prefix);

        let built_source = command_dispatcher
            .backend_source_build(BackendSourceBuildSpec {
                method: BackendSourceBuildMethod::BuildV1(BackendSourceBuildV1Method {
                    editable: self.editable(),
                    dependencies,
                    run_exports,
                    build_prefix: BackendSourceBuildPrefix {
                        platform: self.build_environment.build_platform,
                        prefix: directories.build_prefix,
                        dependencies: build_dependencies,
                        records: build_records
                            .iter()
                            .map(|p| p.repodata_record.clone())
                            .collect(),
                    },
                    host_prefix: BackendSourceBuildPrefix {
                        platform: self.build_environment.host_platform,
                        prefix: directories.host_prefix,
                        dependencies: host_dependencies,
                        records: host_records
                            .iter()
                            .map(|p| p.repodata_record.clone())
                            .collect(),
                    },
                    variant: output.metadata.variant,
                    output_directory: self.output_directory,
                }),
                backend,
                package: self.package,
                source: self.source,
                work_directory,
                channels: self.channels,
                channel_config: self.channel_config,
            })
            .await
            .map_err_with(SourceBuildError::from)?;

        Ok(BuiltPackage {
            output_file: built_source.output_file,
            metadata: CachedBuildSourceInfo {
                globs: built_source.input_globs,
                build: BuildHostEnvironment {
                    packages: build_records,
                },
                host: BuildHostEnvironment {
                    packages: host_records,
                },
                package_build_input_hash: Some(package_build_input_hash),
            },
        })
    }

    /// Little helper function the build a `BuildHostPackage` from expected and
    /// installed records.
    fn extract_prefix_repodata(
        records: Vec<PixiRecord>,
        prefix: Option<InstallPixiEnvironmentResult>,
    ) -> Vec<BuildHostPackage> {
        let Some(prefix) = prefix else {
            return vec![];
        };

        records
            .into_iter()
            .map(|record| match record {
                PixiRecord::Binary(repodata_record) => BuildHostPackage {
                    repodata_record,
                    source: None,
                },
                PixiRecord::Source(source) => {
                    let repodata_record = prefix
                        .resolved_source_records
                        .get(&source.package_record.name)
                        .cloned()
                        .expect("the source record should be present in the result sources");
                    BuildHostPackage {
                        repodata_record,
                        source: Some(source.source),
                    }
                }
            })
            .collect()
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
                installed: vec![], // TODO: To lock build environments, fill this.
                build_environment,
                channels: self.channels.clone(),
                strategy: Default::default(),
                channel_priority: Default::default(),
                exclude_newer: None,
                channel_config: self.channel_config.clone(),
                variants: self.variants.clone(),
                enabled_protocols: self.enabled_protocols.clone(),
            })
            .await
    }
}

pub struct Directories {
    host_prefix: PathBuf,
    build_prefix: PathBuf,
}

impl Directories {
    pub fn new(work_directory: &Path, host_platform: Platform) -> Self {
        const BUILD_DIR: &str = "bld";
        const HOST_ENV_DIR: &str = "host";
        const PLACEHOLDER_TEMPLATE_STR: &str = "_placehold";

        let build_prefix = work_directory.join(BUILD_DIR);
        let host_prefix = if host_platform.is_windows() {
            work_directory.join(HOST_ENV_DIR)
        } else {
            // On non-Windows platforms, the name of the host environment has to be exactly
            // 255 characters long for prefix replacement in rattler build to work
            // correctly. This code constructs a directory name padded with a
            // template string so its exactly 255 characters long.
            //
            // TODO: This is really an implementation detail of how backends are generally
            // implemented, but this code should not really live in pixi.
            const PLACEHOLDER_LENGTH: usize = 255;
            let mut placeholder = String::new();
            while placeholder.len() < PLACEHOLDER_LENGTH {
                placeholder.push_str(PLACEHOLDER_TEMPLATE_STR);
            }
            let placeholder = placeholder
                [0..PLACEHOLDER_LENGTH - work_directory.join(HOST_ENV_DIR).as_os_str().len()]
                .to_string();

            work_directory.join(format!("{HOST_ENV_DIR}{}", placeholder))
        };
        Self {
            host_prefix,
            build_prefix,
        }
    }
}

/// Computes the SHA256 hash of the package at the given path in a separate
/// thread.
async fn compute_package_sha256(
    package_path: &Path,
) -> Result<Sha256Hash, CommandDispatcherError<SourceBuildError>> {
    let path = package_path.to_path_buf();
    simple_spawn_blocking::tokio::run_blocking_task(move || {
        rattler_digest::compute_file_digest::<rattler_digest::Sha256>(&path)
            .map_err(|e| CommandDispatcherError::Failed(SourceBuildError::CalculateSha256(path, e)))
    })
    .await
}

#[derive(Debug, Error, Diagnostic)]
pub enum SourceBuildError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    SourceCheckout(#[from] SourceCheckoutError),

    #[error(transparent)]
    BuildCache(#[from] BuildCacheError),

    #[error("failed to amend run exports: {0}")]
    RunExportsExtraction(#[from] RunExportExtractorError),

    #[error(transparent)]
    CreateWorkDirectory(std::io::Error),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Discovery(#[from] pixi_build_discovery::DiscoveryError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Initialize(#[from] InstantiateBackendError),

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

    #[error("failed to create the build environment directory")]
    CreateBuildEnvironmentDirectory(#[source] std::io::Error),

    #[error("failed to create the host environment directory")]
    CreateHostEnvironmentDirectory(#[source] std::io::Error),

    #[error("failed to install the build environment")]
    InstallBuildEnvironment(#[source] Box<InstallPixiEnvironmentError>),

    #[error("failed to install the host environment")]
    InstallHostEnvironment(#[source] Box<InstallPixiEnvironmentError>),

    #[error(
        "The build backend does not provide the requested output: {subdir}/{name}={version}={build}."
    )]
    MissingOutput {
        subdir: String,
        name: String,
        version: String,
        build: String,
    },

    #[error(
        "The build backend returned a path for the build package ({0}), but the path does not exist."
    )]
    MissingOutputFile(PathBuf),

    #[error("backend returned a dependency on an invalid package name: {0}")]
    InvalidPackageName(String, #[source] InvalidPackageNameError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    BackendBuildError(#[from] BackendSourceBuildError),

    #[error("failed to clean the working directory: {0}")]
    CleanWorkingDirectory(PathBuf, #[source] std::io::Error),

    #[error("moving the built package from {0} to the output directory {1} failed")]
    Move(PathBuf, PathBuf, #[source] MoveError),

    #[error("failed to create the output directory")]
    CreateOutputDirectory(#[source] std::io::Error),

    #[error("failed to read metadata from the output package")]
    ReadIndexJson(#[source] rattler_package_streaming::ExtractError),

    #[error("failed to calculate sha256 hash of {}", .0.display())]
    CalculateSha256(std::path::PathBuf, #[source] std::io::Error),

    #[error("the package does not contain a valid subdir")]
    ConvertSubdir(#[source] ConvertSubdirError),
}

impl From<DependenciesError> for SourceBuildError {
    fn from(value: DependenciesError) -> Self {
        match value {
            DependenciesError::InvalidPackageName(name, error) => {
                SourceBuildError::InvalidPackageName(name, error)
            }
        }
    }
}

impl From<SourceBuildCacheStatusError> for SourceBuildError {
    fn from(value: SourceBuildCacheStatusError) -> Self {
        match value {
            SourceBuildCacheStatusError::BuildCache(err) => SourceBuildError::BuildCache(err),
            SourceBuildCacheStatusError::Discovery(err) => SourceBuildError::Discovery(err),
            SourceBuildCacheStatusError::SourceCheckout(err) => {
                SourceBuildError::SourceCheckout(err)
            }
            SourceBuildCacheStatusError::Cycle => {
                unreachable!("a build time cycle should never happen")
            }
        }
    }
}
