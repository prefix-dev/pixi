use std::{path::Path, sync::Arc};

use async_once_cell::OnceCell as AsyncCell;

use once_cell::sync::OnceCell;

use anyhow::Result;
use std::collections::HashMap;
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
use uv_pypi_types::Requirement;
use uv_python::{Interpreter, PythonEnvironment};
use uv_resolver::{ExcludeNewer, FlatIndex};
use uv_types::{BuildContext, BuildIsolation, HashStrategy};

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
    build_isolation: BuildIsolation<'a>,
    link_mode: uv_install_wheel::linker::LinkMode,
    build_options: &'a BuildOptions,
    hasher: &'a HashStrategy,
    exclude_newer: Option<ExcludeNewer>,
    bounds: LowerBound,
    sources: SourceStrategy,
    concurrency: Concurrency,
    env_variables: HashMap<String, String>,
}

impl<'a> UvBuildDispatchParams<'a> {
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
        build_isolation: BuildIsolation<'a>,
        link_mode: uv_install_wheel::linker::LinkMode,
        build_options: &'a BuildOptions,
        hasher: &'a HashStrategy,
        exclude_newer: Option<ExcludeNewer>,
        bounds: LowerBound,
        sources: SourceStrategy,
        concurrency: Concurrency,
        env_variables: HashMap<String, String>,
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
            build_isolation,
            link_mode,
            build_options,
            hasher,
            exclude_newer,
            bounds,
            sources,
            concurrency,
            env_variables,
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
    pub fn new(
        params: UvBuildDispatchParams<'a>,
        prefix_task: CondaPrefixUpdater<'a>,
        repodata_records: Arc<PixiRecordsByName>,
        interpreter: &'a OnceCell<Interpreter>,
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
            conda_task: None,
            repodata_records,
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
    async fn initialize(&self) -> anyhow::Result<&BuildDispatch> {
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
                    .get_or_try_init(|| Interpreter::query(python_path, &self.cache))?;

                let build_dispatch = BuildDispatch::new(
                    &self.params.client,
                    &self.params.cache,
                    self.params.constraints.clone(),
                    interpreter,
                    &self.params.index_locations,
                    &self.params.flat_index,
                    &self.params.dependency_metadata,
                    // TODO: could use this later to add static metadata
                    self.params.shared_state.clone(),
                    self.params.index_strategy,
                    &self.params.config_settings,
                    self.params.build_isolation.clone(),
                    self.params.link_mode.clone(),
                    &self.params.build_options,
                    &self.params.hasher,
                    self.params.exclude_newer,
                    self.params.bounds,
                    self.params.sources.clone(),
                    self.params.concurrency,
                )
                .with_build_extra_env_vars(self.params.env_variables.clone());

                Ok(build_dispatch)
            })
            .await
    }
}

impl<'a> BuildContext for PixiBuildDispatch<'a> {
    type SourceDistBuilder = SourceBuild;

    fn interpreter(&self) -> &uv_python::Interpreter {
        // set the interpreter
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let result = runtime.block_on(self.initialize()).unwrap();

        self.interpreter.get().unwrap()
    }

    fn cache(&self) -> &uv_cache::Cache {
        &self.cache
    }

    fn git(&self) -> &uv_git::GitResolver {
        &self.git
    }

    fn capabilities(&self) -> &uv_distribution_types::IndexCapabilities {
        &self.capabilities
    }

    fn dependency_metadata(&self) -> &uv_distribution_types::DependencyMetadata {
        &self.dependency_metadata
    }

    fn build_options(&self) -> &uv_configuration::BuildOptions {
        &self.build_options
    }

    fn config_settings(&self) -> &uv_configuration::ConfigSettings {
        &self.config_settings
    }

    fn bounds(&self) -> uv_configuration::LowerBound {
        self.bounds
    }

    fn sources(&self) -> uv_configuration::SourceStrategy {
        self.sources
    }

    fn locations(&self) -> &uv_distribution_types::IndexLocations {
        &self.locations
    }

    async fn resolve<'data>(&'data self, requirements: &'data [Requirement]) -> Result<Resolution> {
        self.initialize().await?.resolve(requirements).await
    }

    async fn install<'data>(
        &'data self,
        resolution: &'data Resolution,
        venv: &'data PythonEnvironment,
    ) -> Result<Vec<CachedDist>> {
        self.initialize().await?.install(resolution, venv).await
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
        self.initialize()
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
