use std::{path::Path, sync::Arc};

use async_once_cell::OnceCell as AsyncCell;

use once_cell::sync::OnceCell;

use anyhow::Result;
use pixi_manifest::EnvironmentName;
use pixi_uv_conversions::{isolated_names_to_packages, names_to_build_isolation};
use std::collections::HashMap;
use tokio::runtime::Handle;
use uv_build_frontend::SourceBuild;
use uv_cache::Cache;
use uv_client::RegistryClient;
use uv_configuration::{
    BuildKind, BuildOptions, BuildOutput, Concurrency, ConfigSettings, Constraints, IndexStrategy,
    LowerBound, SourceStrategy,
};
use uv_dispatch::{BuildDispatch, SharedState};
use uv_distribution_types::{
    CachedDist, DependencyMetadata, IndexLocations, Resolution, SourceDist,
};
use uv_pep508::PackageName;
use uv_pypi_types::Requirement;
use uv_python::{Interpreter, PythonEnvironment};
use uv_resolver::{ExcludeNewer, FlatIndex};
use uv_types::{BuildContext, HashStrategy};

use crate::{
    activation::CurrentEnvVarBehavior,
    project::{get_activated_environment_variables, Environment, EnvironmentVars},
};

use super::{conda_prefix_updater::CondaPrefixUpdated, CondaPrefixUpdater, PixiRecordsByName};

/// This structure holds all the parameters needed to create a `BuildContext` uv implementation.
pub struct UvBuildDispatchParams<'a> {
    client: &'a RegistryClient,
    cache: &'a Cache,
    constraints: Constraints,
    index_locations: &'a IndexLocations,
    flat_index: &'a FlatIndex,
    dependency_metadata: &'a DependencyMetadata,
    shared_state: SharedState,
    index_strategy: IndexStrategy,
    config_settings: &'a ConfigSettings,
    link_mode: uv_install_wheel::linker::LinkMode,
    build_options: &'a BuildOptions,
    hasher: &'a HashStrategy,
    exclude_newer: Option<ExcludeNewer>,
    bounds: LowerBound,
    sources: SourceStrategy,
    concurrency: Concurrency,
}

impl<'a> UvBuildDispatchParams<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: &'a RegistryClient,
        cache: &'a Cache,
        constraints: Constraints,
        index_locations: &'a IndexLocations,
        flat_index: &'a FlatIndex,
        dependency_metadata: &'a DependencyMetadata,
        shared_state: SharedState,
        index_strategy: IndexStrategy,
        config_settings: &'a ConfigSettings,
        link_mode: uv_install_wheel::linker::LinkMode,
        build_options: &'a BuildOptions,
        hasher: &'a HashStrategy,
        exclude_newer: Option<ExcludeNewer>,
        bounds: LowerBound,
        sources: SourceStrategy,
        concurrency: Concurrency,
    ) -> Self {
        Self {
            client,
            cache,
            constraints,
            index_locations,
            flat_index,
            dependency_metadata,
            shared_state,
            index_strategy,
            config_settings,
            link_mode,
            build_options,
            hasher,
            exclude_newer,
            bounds,
            sources,
            concurrency,
        }
    }
}

/// Something that implements the `BuildContext` trait.
pub struct PixiBuildDispatch<'a> {
    pub params: UvBuildDispatchParams<'a>,
    pub prefix_task: CondaPrefixUpdater<'a>,
    pub repodata_records: Arc<PixiRecordsByName>,

    pub build_dispatch: AsyncCell<BuildDispatch<'a>>,
    // we need to tie the interpreter to the build dispatch
    pub interpreter: &'a OnceCell<Interpreter>,

    // if we create a new conda prefix, we need to store the task result
    // so we could reuse it later
    pub conda_task: Option<CondaPrefixUpdated>,

    // project environment variables
    // this is used to get the activated environment variables
    pub project_env_vars: HashMap<EnvironmentName, EnvironmentVars>,
    pub environment: Environment<'a>,

    // what pkgs we dont need to activate
    pub no_build_isolation: Option<Vec<String>>,

    // non isolated packages
    pub non_isolated_packages: &'a OnceCell<Option<Vec<PackageName>>>,

    // python environment
    pub python_env: &'a OnceCell<PythonEnvironment>,

    // values that can be passed in the BuildContext trait
    cache: &'a uv_cache::Cache,
    git: &'a uv_git::GitResolver,
    capabilities: &'a uv_distribution_types::IndexCapabilities,
    dependency_metadata: &'a uv_distribution_types::DependencyMetadata,
    build_options: &'a uv_configuration::BuildOptions,
    config_settings: &'a uv_configuration::ConfigSettings,
    bounds: uv_configuration::LowerBound,
    sources: uv_configuration::SourceStrategy,
    locations: &'a uv_distribution_types::IndexLocations,
}

impl<'a> PixiBuildDispatch<'a> {
    /// Create a new `PixiBuildDispatch` instance.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        params: UvBuildDispatchParams<'a>,
        prefix_task: CondaPrefixUpdater<'a>,
        project_env_vars: HashMap<EnvironmentName, EnvironmentVars>,
        environment: Environment<'a>,
        repodata_records: Arc<PixiRecordsByName>,
        interpreter: &'a OnceCell<Interpreter>,
        non_isolated_packages: &'a OnceCell<Option<Vec<PackageName>>>,
        python_env: &'a OnceCell<PythonEnvironment>,
        no_build_isolation: Option<Vec<String>>,
        cache: &'a uv_cache::Cache,
        git: &'a uv_git::GitResolver,
        capabilities: &'a uv_distribution_types::IndexCapabilities,
        dependency_metadata: &'a uv_distribution_types::DependencyMetadata,
        build_options: &'a uv_configuration::BuildOptions,
        config_settings: &'a uv_configuration::ConfigSettings,
        bounds: uv_configuration::LowerBound,
        sources: uv_configuration::SourceStrategy,
        locations: &'a uv_distribution_types::IndexLocations,
    ) -> Self {
        Self {
            params,
            prefix_task,
            interpreter,
            non_isolated_packages,
            python_env,
            conda_task: None,
            project_env_vars,
            environment,
            repodata_records,
            no_build_isolation,
            cache,
            git,
            capabilities,
            dependency_metadata,
            build_options,
            config_settings,
            bounds,
            sources,
            locations,
            build_dispatch: AsyncCell::new(),
        }
    }

    /// Lazy initialization of the `BuildDispatch`.
    async fn get_or_try_init(&self) -> anyhow::Result<&BuildDispatch> {
        self.build_dispatch
            .get_or_try_init(async {
                tracing::debug!(
                    "installing conda prefix {} for solving the pypi sdist requirements",
                    self.prefix_task.group.name().as_str()
                );
                let prefix = self
                    .prefix_task
                    .update(self.repodata_records.clone())
                    .await
                    .map_err(|err| {
                        anyhow::anyhow!(err).context("failed to install conda prefix")
                    })?;

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
                .map_err(|err| {
                    anyhow::anyhow!(err).context("failed to get activated environment variables")
                })?;

                let python_path = prefix
                    .python_status
                    .location()
                    .map(|path| prefix.prefix.root().join(path))
                    .ok_or_else(|| {
                        anyhow::anyhow!(format!(
                            "missing python interpreter from conda prefix {}. \n {}",
                            prefix.prefix.root().display(),
                            "Use `pixi add python` to install the latest python interpreter.",
                        ))
                    })?;

                let interpreter = self
                    .interpreter
                    .get_or_try_init(|| Interpreter::query(python_path, self.cache))?;

                let env = self
                    .python_env
                    .get_or_init(|| PythonEnvironment::from_interpreter(interpreter.clone()));

                let non_isolated_packages = self.non_isolated_packages.get_or_try_init(|| {
                    isolated_names_to_packages(self.no_build_isolation.as_deref()).map_err(|err| {
                        anyhow::anyhow!(err).context("failed to get non isolated packages")
                    })
                })?;

                let build_isolation =
                    names_to_build_isolation(non_isolated_packages.as_deref(), env);

                let build_dispatch = BuildDispatch::new(
                    self.params.client,
                    self.params.cache,
                    self.params.constraints.clone(),
                    interpreter,
                    self.params.index_locations,
                    self.params.flat_index,
                    self.params.dependency_metadata,
                    // TODO: could use this later to add static metadata
                    self.params.shared_state.clone(),
                    self.params.index_strategy,
                    self.params.config_settings,
                    build_isolation,
                    self.params.link_mode,
                    self.params.build_options,
                    self.params.hasher,
                    self.params.exclude_newer,
                    self.params.bounds,
                    self.params.sources,
                    self.params.concurrency,
                )
                .with_build_extra_env_vars(env_vars);

                Ok(build_dispatch)
            })
            .await
    }
}

impl BuildContext for PixiBuildDispatch<'_> {
    type SourceDistBuilder = SourceBuild;

    fn interpreter(&self) -> &uv_python::Interpreter {
        // In most cases the interpreter should be initialized, because one of the other trait
        // methods will have been called
        // But in case it is not, we will initialize it here
        //
        // Even though intitalize does not initialize twice, we skip the codepath because the initialization takes time
        if self.interpreter.get().is_none() {
            // This will usually be called from the multi-threaded runtime, but there might be tests
            // that calls this in the current thread runtime.
            // In the current thread runtime we cannot use `block_in_place` as it will pani
            let handle = Handle::current();
            match handle.runtime_flavor() {
                tokio::runtime::RuntimeFlavor::CurrentThread => {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("failed to initialize the runtime ");
                    runtime
                        .block_on(self.get_or_try_init())
                        .expect("failed to initialize the build dispatch");
                }
                // Others are multi-threaded runtimes
                _ => {
                    tokio::task::block_in_place(move || {
                        handle
                            .block_on(self.get_or_try_init())
                            .expect("failed to initialize build dispatch");
                    });
                }
            }
        }
        self.interpreter
            .get()
            .expect("python interpreter not initialized, this is a programming error")
    }

    fn cache(&self) -> &uv_cache::Cache {
        self.cache
    }

    fn git(&self) -> &uv_git::GitResolver {
        self.git
    }

    fn capabilities(&self) -> &uv_distribution_types::IndexCapabilities {
        self.capabilities
    }

    fn dependency_metadata(&self) -> &uv_distribution_types::DependencyMetadata {
        self.dependency_metadata
    }

    fn build_options(&self) -> &uv_configuration::BuildOptions {
        self.build_options
    }

    fn config_settings(&self) -> &uv_configuration::ConfigSettings {
        self.config_settings
    }

    fn bounds(&self) -> uv_configuration::LowerBound {
        self.bounds
    }

    fn sources(&self) -> uv_configuration::SourceStrategy {
        self.sources
    }

    fn locations(&self) -> &uv_distribution_types::IndexLocations {
        self.locations
    }

    async fn resolve<'data>(&'data self, requirements: &'data [Requirement]) -> Result<Resolution> {
        self.get_or_try_init().await?.resolve(requirements).await
    }

    async fn install<'data>(
        &'data self,
        resolution: &'data Resolution,
        venv: &'data PythonEnvironment,
    ) -> Result<Vec<CachedDist>> {
        self.get_or_try_init()
            .await?
            .install(resolution, venv)
            .await
    }

    async fn setup_build<'data>(
        &'data self,
        source: &'data Path,
        subdirectory: Option<&'data Path>,
        install_path: &'data Path,
        version_id: Option<String>,
        dist: Option<&'data SourceDist>,
        sources: SourceStrategy,
        build_kind: BuildKind,
        build_output: BuildOutput,
    ) -> Result<SourceBuild> {
        self.get_or_try_init()
            .await?
            .setup_build(
                source,
                subdirectory,
                install_path,
                version_id,
                dist,
                sources,
                build_kind,
                build_output,
            )
            .await
    }
}
