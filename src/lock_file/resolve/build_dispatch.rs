//! This module contains an implementation of the `BuildContext` trait for the
//! `LazyBuildDispatch` trait. This is mainly to be able to initialize the conda
//! prefix for PyPI resolving on demand. This is needed because the conda prefix
//! is a heavy operation and we want to avoid initializing it. And we do not
//! need to initialize it if we are not resolving PyPI source dependencies. With
//! this implementation we only initialize a prefix once uv requests some
//! operation that actually needs this prefix.
//!
//! This is especially prudent to do when we have multiple environments, which
//! translates into multiple prefixes, that all need to be initialized.
//! Previously we would initialize all prefixes upfront, but this is not needed
//! and can also sometimes not be done for each platform. Using this
//! implementation we can solve for a lot more platforms than we could before.
//!
//! The main struct of interest is the [`LazyBuildDispatch`] struct which holds
//! the parameters needed to create a `BuildContext` uv implementation.
//! and holds struct that is used to instantiate the conda prefix when its
//! needed.
use std::cell::Cell;
use std::{collections::HashMap, path::Path};

use crate::environment::{CondaPrefixUpdated, CondaPrefixUpdater};
use crate::{
    activation::CurrentEnvVarBehavior,
    workspace::{Environment, EnvironmentVars, get_activated_environment_variables},
};
use async_once_cell::OnceCell as AsyncCell;
use once_cell::sync::OnceCell;
use pixi_manifest::EnvironmentName;
use pixi_manifest::pypi::pypi_options::NoBuildIsolation;
use pixi_record::PixiRecord;
use pixi_uv_conversions::BuildIsolation;
use uv_build_frontend::SourceBuild;
use uv_cache::Cache;
use uv_client::RegistryClient;
use uv_configuration::{
    BuildKind, BuildOptions, BuildOutput, Concurrency, ConfigSettings, Constraints, IndexStrategy,
    PackageConfigSettings, SourceStrategy,
};
use uv_dispatch::{BuildDispatch, BuildDispatchError, SharedState};
use uv_distribution_filename::DistFilename;
use uv_distribution_types::Requirement;
use uv_distribution_types::{
    CachedDist, DependencyMetadata, ExtraBuildRequires, IndexLocations, IsBuildBackendError,
    Resolution, SourceDist,
};
use uv_install_wheel::LinkMode;
use uv_python::{Interpreter, InterpreterError, PythonEnvironment};
use uv_resolver::{ExcludeNewer, FlatIndex};
use uv_types::{BuildArena, BuildContext, BuildStack, HashStrategy};
use uv_workspace::WorkspaceCache;

/// This structure holds all the parameters needed to create a `BuildContext` uv implementation.
pub struct UvBuildDispatchParams<'a> {
    client: &'a RegistryClient,
    cache: &'a Cache,
    index_locations: &'a IndexLocations,
    flat_index: &'a FlatIndex,
    dependency_metadata: &'a DependencyMetadata,
    config_settings: &'a ConfigSettings,
    package_config_settings: PackageConfigSettings,
    build_options: &'a BuildOptions,
    extra_build_requires: ExtraBuildRequires,
    hasher: &'a HashStrategy,
    index_strategy: IndexStrategy,
    constraints: Constraints,
    shared_state: SharedState,
    link_mode: uv_install_wheel::LinkMode,
    exclude_newer: Option<ExcludeNewer>,
    sources: SourceStrategy,
    concurrency: Concurrency,
    preview: uv_configuration::Preview,
    workspace_cache: WorkspaceCache,
}

impl<'a> UvBuildDispatchParams<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: &'a RegistryClient,
        cache: &'a Cache,
        index_locations: &'a IndexLocations,
        flat_index: &'a FlatIndex,
        dependency_metadata: &'a DependencyMetadata,
        config_settings: &'a ConfigSettings,
        build_options: &'a BuildOptions,
        hasher: &'a HashStrategy,
    ) -> Self {
        Self {
            client,
            cache,
            index_locations,
            flat_index,
            dependency_metadata,
            config_settings,
            package_config_settings: PackageConfigSettings::default(),
            build_options,
            extra_build_requires: ExtraBuildRequires::default(),
            hasher,
            index_strategy: IndexStrategy::default(),
            shared_state: SharedState::default(),
            link_mode: LinkMode::default(),
            constraints: Constraints::default(),
            exclude_newer: None,
            sources: SourceStrategy::default(),
            concurrency: Concurrency::default(),
            preview: uv_configuration::Preview::default(),
            workspace_cache: WorkspaceCache::default(),
        }
    }

    /// Set the index strategy for the build dispatch.
    pub fn with_index_strategy(mut self, index_strategy: IndexStrategy) -> Self {
        self.index_strategy = index_strategy;
        self
    }

    /// Set the shared state for the build dispatch
    pub fn with_shared_state(mut self, shared_state: SharedState) -> Self {
        self.shared_state = shared_state;
        self
    }

    /// Set the source strategy for the build dispatch
    pub fn with_source_strategy(mut self, sources: SourceStrategy) -> Self {
        self.sources = sources;
        self
    }

    /// Set the concurrency amount for the build dispatch
    pub fn with_concurrency(mut self, concurrency: Concurrency) -> Self {
        self.concurrency = concurrency;
        self
    }

    /// Set the link mode for the build dispatch
    #[allow(dead_code)]
    pub fn with_link_mode(mut self, link_mode: LinkMode) -> Self {
        self.link_mode = link_mode;
        self
    }

    /// Set the constraints for the build dispatch
    #[allow(dead_code)]
    pub fn with_constraints(mut self, constraints: Constraints) -> Self {
        self.constraints = constraints;
        self
    }

    /// Set the exclude newer options for the build dispatch
    pub fn with_exclude_newer(mut self, exclude_newer: ExcludeNewer) -> Self {
        self.exclude_newer = Some(exclude_newer);
        self
    }

    #[allow(dead_code)]
    pub fn with_preview_mode(mut self, preview: uv_configuration::Preview) -> Self {
        self.preview = preview;
        self
    }

    pub fn with_workspace_cache(mut self, workspace_cache: WorkspaceCache) -> Self {
        self.workspace_cache = workspace_cache;
        self
    }

    #[allow(dead_code)]
    pub fn with_package_config_settings(
        mut self,
        package_config_settings: PackageConfigSettings,
    ) -> Self {
        self.package_config_settings = package_config_settings;
        self
    }

    #[allow(dead_code)]
    pub fn with_extra_build_requires(mut self, extra_build_requires: ExtraBuildRequires) -> Self {
        self.extra_build_requires = extra_build_requires;
        self
    }
}

/// Handles the lazy initialization of a build dispatch.
///
/// A build dispatch is used to manage building Python packages from source,
/// including setting up build environments, dependencies, and executing builds.
///
/// This struct helps manage resources needed for build dispatch that may need
/// to be initialized on-demand rather than upfront.
///
/// Both the [`BuildDispatch`] and the conda prefix are instantiated on demand.
pub struct LazyBuildDispatch<'a> {
    pub params: UvBuildDispatchParams<'a>,
    pub prefix_updater: CondaPrefixUpdater,
    pub repodata_records: Cell<Option<miette::Result<Vec<PixiRecord>>>>,

    pub build_dispatch: AsyncCell<BuildDispatch<'a>>,

    // if we create a new conda prefix, we need to store the task result
    // so that we can reuse it later
    pub conda_task: Option<CondaPrefixUpdated>,

    // project environment variables
    // this is used to get the activated environment variables
    pub project_env_vars: HashMap<EnvironmentName, EnvironmentVars>,
    pub environment: Environment<'a>,

    // what pkgs we dont need to activate
    pub no_build_isolation: NoBuildIsolation,

    // we need to tie the interpreter to the build dispatch
    pub lazy_deps: &'a LazyBuildDispatchDependencies,

    /// Whether to disallow installing the conda prefix.
    pub disallow_install_conda_prefix: bool,

    workspace_cache: WorkspaceCache,
}

/// These are resources for the [`BuildDispatch`] that need to be lazily
/// initialized. along with the build dispatch.
///
/// This needs to be passed in externally or there will be problems with the
/// borrows being shorter than the lifetime of the `BuildDispatch`, and we are
/// returning the references.
#[derive(Default)]
pub struct LazyBuildDispatchDependencies {
    /// The initialized python interpreter
    interpreter: OnceCell<Interpreter>,
    /// The non isolated packages
    non_isolated_packages: OnceCell<BuildIsolation>,
    /// The python environment
    python_env: OnceCell<PythonEnvironment>,
    /// The constraints for dependency resolution
    constraints: OnceCell<Constraints>,
    /// Extra build requirements
    extra_build_requires: OnceCell<ExtraBuildRequires>,
    /// Package-specific configuration settings
    package_config_settings: OnceCell<PackageConfigSettings>,
}

#[derive(Debug, thiserror::Error, miette::Diagnostic)]
enum LazyBuildDispatchError {
    #[error(
        "installation of conda environment is required to solve PyPI source dependencies but `--no-install` flag has been set"
    )]
    InstallationRequiredButDisallowed,
    #[error(transparent)]
    #[diagnostic(transparent)]
    InitializationError(Box<dyn miette::Diagnostic + Send + Sync>),
    #[error(transparent)]
    ConversionError(#[from] pixi_uv_conversions::ConversionError),
    #[error(transparent)]
    Uv(#[from] BuildDispatchError),
    #[error(transparent)]
    UvFrontend(#[from] uv_build_frontend::Error),
    #[error("failed to query interpreter in instantiated prefix")]
    QueryInterpreterError(#[from] InterpreterError),
    #[error(
        "missing python interpreter from conda prefix: {prefix},\nUse `pixi add python` to install the latest python interpreter."
    )]
    PythonMissingError { prefix: String },
}

impl IsBuildBackendError for LazyBuildDispatchError {
    fn is_build_backend_error(&self) -> bool {
        false
    }
}

impl<'a> LazyBuildDispatch<'a> {
    /// Create a new `PixiBuildDispatch` instance.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        params: UvBuildDispatchParams<'a>,
        prefix_updater: CondaPrefixUpdater,
        project_env_vars: HashMap<EnvironmentName, EnvironmentVars>,
        environment: Environment<'a>,
        repodata_records: miette::Result<Vec<PixiRecord>>,
        no_build_isolation: NoBuildIsolation,
        lazy_deps: &'a LazyBuildDispatchDependencies,
        disallow_install_conda_prefix: bool,
    ) -> Self {
        Self {
            params,
            prefix_updater,
            conda_task: None,
            project_env_vars,
            environment,
            repodata_records: Cell::new(Some(repodata_records)),
            no_build_isolation,
            build_dispatch: AsyncCell::new(),
            lazy_deps,
            disallow_install_conda_prefix,
            workspace_cache: WorkspaceCache::default(),
        }
    }

    /// Lazy initialization of the `BuildDispatch`. This also implies
    /// initializing the conda prefix.
    async fn get_or_try_init(&self) -> Result<&BuildDispatch<'a>, LazyBuildDispatchError> {
        Box::pin(self.build_dispatch.get_or_try_init(async {
            // Disallow installing if the flag is set.
            if self.disallow_install_conda_prefix {
                return Err(LazyBuildDispatchError::InstallationRequiredButDisallowed);
            }
            tracing::debug!(
                "PyPI solve requires instantiation of conda prefix for '{}'",
                self.prefix_updater.name().as_str()
            );

            let repodata_records = self
                .repodata_records
                .replace(None)
                .expect("this function cannot be called twice")
                .map_err(|err| LazyBuildDispatchError::InitializationError(err.into()))?;

            let prefix = self
                .prefix_updater
                .update(repodata_records.to_vec(), None)
                .await
                .map_err(|err| LazyBuildDispatchError::InitializationError(err.into()))?;

            // get the activation vars
            let env_vars = get_activated_environment_variables(
                &self.project_env_vars,
                &self.environment,
                CurrentEnvVarBehavior::Exclude,
                None,
                false,
                false,
            )
            .await
            .map_err(|err| LazyBuildDispatchError::InitializationError(err.into()))?;

            let python_path = prefix
                .python_status
                .location()
                .map(|path| prefix.prefix.root().join(path))
                .ok_or_else(|| LazyBuildDispatchError::PythonMissingError {
                    prefix: prefix.prefix.root().display().to_string(),
                })?;

            let interpreter = self
                .lazy_deps
                .interpreter
                .get_or_try_init(|| Interpreter::query(python_path, self.cache()))
                .map_err(LazyBuildDispatchError::from)?;

            let non_isolated_packages = self
                .lazy_deps
                .non_isolated_packages
                .get_or_try_init(|| BuildIsolation::try_from(self.no_build_isolation.clone()))
                .map_err(LazyBuildDispatchError::from)?;

            let build_isolation = non_isolated_packages.to_uv_with(|| {
                self.lazy_deps
                    .python_env
                    .get_or_init(|| PythonEnvironment::from_interpreter(interpreter.clone()))
            });

            let constraints = self
                .lazy_deps
                .constraints
                .get_or_init(|| self.params.constraints.clone());

            let extra_build_requires = self
                .lazy_deps
                .extra_build_requires
                .get_or_init(|| self.params.extra_build_requires.clone());

            let package_config_settings = self
                .lazy_deps
                .package_config_settings
                .get_or_init(|| self.params.package_config_settings.clone());

            let build_dispatch = BuildDispatch::new(
                self.params.client,
                self.params.cache,
                constraints,
                interpreter,
                self.params.index_locations,
                self.params.flat_index,
                self.params.dependency_metadata,
                self.params.shared_state.clone(),
                self.params.index_strategy,
                self.params.config_settings,
                package_config_settings,
                build_isolation,
                extra_build_requires,
                self.params.link_mode,
                self.params.build_options,
                self.params.hasher,
                self.params.exclude_newer.clone().unwrap_or_default(),
                self.params.sources,
                self.params.workspace_cache.clone(),
                self.params.concurrency,
                self.params.preview,
            )
            .with_build_extra_env_vars(env_vars);

            Ok(build_dispatch)
        }))
        .await
    }
}

impl BuildContext for LazyBuildDispatch<'_> {
    type SourceDistBuilder = SourceBuild;

    async fn interpreter(&self) -> &uv_python::Interpreter {
        // In most cases the interpreter should be initialized, because one of the other
        // trait methods will have been called
        // But in case it is not, we will initialize it here
        //
        // Even though initialize does not initialize twice, we check it beforehand
        // because the initialization takes time
        self.get_or_try_init()
            .await
            .expect("could not initialize build dispatch correctly")
            .interpreter()
            .await
    }

    fn cache(&self) -> &uv_cache::Cache {
        self.params.cache
    }

    fn git(&self) -> &uv_git::GitResolver {
        self.params.shared_state.git()
    }

    fn capabilities(&self) -> &uv_distribution_types::IndexCapabilities {
        self.params.shared_state.capabilities()
    }

    fn dependency_metadata(&self) -> &uv_distribution_types::DependencyMetadata {
        self.params.dependency_metadata
    }

    fn build_options(&self) -> &uv_configuration::BuildOptions {
        self.params.build_options
    }

    fn config_settings(&self) -> &uv_configuration::ConfigSettings {
        self.params.config_settings
    }

    fn sources(&self) -> uv_configuration::SourceStrategy {
        self.params.sources
    }

    fn locations(&self) -> &uv_distribution_types::IndexLocations {
        self.params.index_locations
    }

    async fn resolve<'a>(
        &'a self,
        requirements: &'a [Requirement],
        build_stack: &'a BuildStack,
    ) -> Result<Resolution, impl IsBuildBackendError> {
        let dispatch = self.get_or_try_init().await?;
        dispatch
            .resolve(requirements, build_stack)
            .await
            .map_err(LazyBuildDispatchError::Uv)
    }

    async fn install<'a>(
        &'a self,
        resolution: &'a Resolution,
        venv: &'a PythonEnvironment,
        build_stack: &'a BuildStack,
    ) -> Result<Vec<CachedDist>, impl IsBuildBackendError> {
        let dispatch = self.get_or_try_init().await?;
        dispatch
            .install(resolution, venv, build_stack)
            .await
            .map_err(LazyBuildDispatchError::Uv)
    }

    async fn setup_build<'a>(
        &'a self,
        source: &'a Path,
        subdirectory: Option<&'a Path>,
        install_path: &'a Path,
        version_id: Option<&'a str>,
        dist: Option<&'a SourceDist>,
        sources: SourceStrategy,
        build_kind: BuildKind,
        build_output: BuildOutput,
        build_stack: BuildStack,
    ) -> Result<Self::SourceDistBuilder, impl IsBuildBackendError> {
        let dispatch = self.get_or_try_init().await?;
        dispatch
            .setup_build(
                source,
                subdirectory,
                install_path,
                version_id,
                dist,
                sources,
                build_kind,
                build_output,
                build_stack,
            )
            .await
            .map_err(LazyBuildDispatchError::from)
    }

    async fn direct_build<'a>(
        &'a self,
        source: &'a Path,
        subdirectory: Option<&'a Path>,
        output_dir: &'a Path,
        build_kind: BuildKind,
        version_id: Option<&'a str>,
    ) -> Result<Option<DistFilename>, impl IsBuildBackendError> {
        let dispatch = self.get_or_try_init().await?;
        dispatch
            .direct_build(source, subdirectory, output_dir, build_kind, version_id)
            .await
            .map_err(LazyBuildDispatchError::from)
    }

    /// Workspace discovery caching.
    fn workspace_cache(&self) -> &WorkspaceCache {
        &self.workspace_cache
    }

    fn build_arena(&self) -> &BuildArena<Self::SourceDistBuilder> {
        // Get the inner build dispatch and delegate to its build_arena method
        self.build_dispatch
            .get()
            .expect("build dispatch not initialized, this is a programming error")
            .build_arena()
    }

    fn config_settings_package(&self) -> &uv_configuration::PackageConfigSettings {
        self.lazy_deps
            .package_config_settings
            .get()
            .expect("package config settings not initialized, this is a programming error")
    }

    fn extra_build_requires(&self) -> &uv_distribution_types::ExtraBuildRequires {
        self.lazy_deps
            .extra_build_requires
            .get()
            .expect("extra build requires not initialized, this is a programming error")
    }
}
