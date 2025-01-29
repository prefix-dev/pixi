use std::{
    cell::{OnceCell, Ref, RefCell},
    path::Path,
    rc::Rc,
    sync::{Arc, Mutex},
};

use async_once_cell::OnceCell as AsyncCell;

// use ahash::HashMap;
use anyhow::Result;
use miette::IntoDiagnostic;
use std::collections::HashMap;
use tokio::runtime::Runtime;
use uv_build_frontend::{SourceBuild, SourceBuildContext};
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

use super::{
    update::{CondaPrefixUpdated, PrefixTask, TaskResult},
    PixiRecordsByName,
};

/// Represents the state of the `PixiBuildDispatch`.
/// It can be uninitialized or lazy initialized
/// at the first call to some of `BuildContext` methods.
/// like `resolve`, `install`, `setup_build` or `interpreter`.
// pub enum PixiBuildDispatchState<'a> {
//     Uninitialized {
//         client: &'a RegistryClient,
//         cache: &'a Cache,
//         constraints: Constraints,
//         index_locations: &'a IndexLocations,
//         flat_index: &'a FlatIndex,
//         dependency_metadata: &'a DependencyMetadata,
//         shared_state: SharedState,
//         index_strategy: IndexStrategy,
//         config_settings: &'a ConfigSettings,
//         build_isolation: BuildIsolation<'a>,
//         link_mode: uv_install_wheel::linker::LinkMode,
//         build_options: &'a BuildOptions,
//         hasher: &'a HashStrategy,
//         exclude_newer: Option<ExcludeNewer>,
//         bounds: LowerBound,
//         sources: SourceStrategy,
//         concurrency: Concurrency,
//         env_variables: HashMap<String, String>,
//     },
//     Initialized {
//         inner: BuildDispatch<'a>,
//     },
// }

pub struct PixiBuildDispatchState<'a> {
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

impl<'a> PixiBuildDispatchState<'a> {
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

    // pub fn is_initialized(&self) -> bool {
    //     matches!(&self, PixiBuildDispatchState::Initialized { .. })
    // }

    // pub fn get_initialized(&self) -> miette::Result<&BuildDispatch<'a>> {
    //     match &self {
    //         PixiBuildDispatchState::Uninitialized { .. } => {
    //             miette::bail!("PixiBuildDispatch is not initialized yet")
    //         }
    //         PixiBuildDispatchState::Initialized { inner, .. } => Ok(inner),
    //     }
    // }
}

/// Something that implements the `BuildContext` trait.
pub struct PixiBuildDispatch<'a> {
    pub state: PixiBuildDispatchState<'a>,
    pub prefix_task: PrefixTask<'a>,
    pub repodata_records: Arc<PixiRecordsByName>,

    pub build_dispatch: AsyncCell<BuildDispatch<'a>>,
    // we need to tie the interpreter to the build dispatch
    pub interpreter: &'a OnceCell<Interpreter>,

    // if we create a new conda prefix, we need to store the task result
    // so we could reuse it later
    pub conda_task: Option<CondaPrefixUpdated>,

    // interpreter: RefCell<Option<Interpreter>>,
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
        // inner: BuildDispatch<'a>,
        state: PixiBuildDispatchState<'a>,
        prefix_task: PrefixTask<'a>,
        repodata_records: Arc<PixiRecordsByName>,
        interpreter: &'a OnceCell<Interpreter>,
        // this can be set directly, for a later build dispatch instantiatne
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
            // inner,
            state: state.into(),
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

    // fn set_interpreter(&self, interpreter: Interpreter) {
    //     self.interpreter.get_or_init(|| Some(interpreter)).unwrap().replace(interpreter);
    // }

    /// Lazy initialization of the `BuildDispatch`.
    async fn initialize(&self) -> miette::Result<()> {
        self.build_dispatch
            .get_or_init(async {
                let prefix = self
                    .prefix_task
                    .clone()
                    .spawn(None, futures::future::ready(self.repodata_records.clone()))
                    .await
                    .unwrap();

                let python_path = prefix
                    .python_status
                    .location()
                    .map(|path| prefix.prefix.root().join(path))
                    .ok_or_else(|| {
                        miette::miette!(
                            help =
                                "Use `pixi add python` to install the latest python interpreter.",
                            "missing python interpreter from environment"
                        )
                    })
                    .unwrap();

                // let interpreter =

                // self.interpreter.replace(Some(interpreter));

                // self.set_interpreter(interpreter);

                eprintln!(
                    "========================= SETING INTERPRETER {}",
                    python_path.display()
                );

                let interpreter = self
                    .interpreter
                    .get_or_init(|| Interpreter::query(python_path, &self.cache).unwrap());

                // self.interpreter.replace(interpreter);

                // let interpreter = self.interpreter.lock().unwrap().as_ref().unwrap();

                let build_dispatch = BuildDispatch::new(
                    &self.state.client,
                    &self.state.cache,
                    self.state.constraints.clone(),
                    interpreter,
                    &self.state.index_locations,
                    &self.state.flat_index,
                    &self.state.dependency_metadata,
                    // TODO: could use this later to add static metadata
                    self.state.shared_state.clone(),
                    self.state.index_strategy,
                    &self.state.config_settings,
                    self.state.build_isolation.clone(),
                    self.state.link_mode.clone(),
                    &self.state.build_options,
                    &self.state.hasher,
                    self.state.exclude_newer,
                    self.state.bounds,
                    self.state.sources.clone(),
                    self.state.concurrency,
                )
                .with_build_extra_env_vars(self.state.env_variables.clone());

                // let state = PixiBuildDispatchState::Initialized {
                //     inner: build_dispatch,
                // };

                build_dispatch
            })
            .await;

        Ok(())
    }
}

impl<'a> BuildContext for PixiBuildDispatch<'a> {
    type SourceDistBuilder = SourceBuild;

    fn interpreter(&self) -> &uv_python::Interpreter {
        // set the interperet
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let result = runtime.block_on(self.initialize());

        // result.unwrap().interpreter()

        self.interpreter.get().unwrap()
        // self.inner.interpreter()
    }

    fn cache(&self) -> &uv_cache::Cache {
        // self.inner.cache()
        &self.cache
    }

    fn git(&self) -> &uv_git::GitResolver {
        // self.inner.git()
        &self.git
    }

    fn capabilities(&self) -> &uv_distribution_types::IndexCapabilities {
        // self.inner.capabilities()
        &self.capabilities
    }

    fn dependency_metadata(&self) -> &uv_distribution_types::DependencyMetadata {
        // self.inner.dependency_metadata()
        &self.dependency_metadata
    }

    fn build_options(&self) -> &uv_configuration::BuildOptions {
        // self.inner.build_options()
        &self.build_options
    }

    fn config_settings(&self) -> &uv_configuration::ConfigSettings {
        // self.inner.config_settings()
        &self.config_settings
    }

    fn bounds(&self) -> uv_configuration::LowerBound {
        // self.inner.bounds()
        self.bounds
    }

    fn sources(&self) -> uv_configuration::SourceStrategy {
        // self.inner.sources()
        self.sources
    }

    fn locations(&self) -> &uv_distribution_types::IndexLocations {
        // self.inner.locations()
        &self.locations
    }

    async fn resolve<'data>(&'data self, requirements: &'data [Requirement]) -> Result<Resolution> {
        // self.inner.resolve(requirements).await
        self.initialize().await.unwrap();

        self.build_dispatch
            .get()
            .expect("we already init it before")
            .resolve(requirements)
            .await
    }

    async fn install<'data>(
        &'data self,
        resolution: &'data Resolution,
        venv: &'data PythonEnvironment,
    ) -> Result<Vec<CachedDist>> {
        // self.inner.install(resolution, venv).await
        self.initialize().await.unwrap();

        self.build_dispatch
            .get()
            .expect("we already init it before")
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
        eprintln!("========================= SETING SETUP BUILD ");
        tracing::debug!("========================= SETING SETUP BUILD ");
        // self.prefix_task
        //     .clone()
        //     .spawn(None, futures::future::ready(self.repodata_records.clone()))
        //     .await
        //     .unwrap();
        self.initialize().await.unwrap();

        tracing::debug!("========================= FULL PREFIX SET ");
        // panic!("Not implemented");
        self.build_dispatch
            .get()
            .expect("we already init it before")
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
        // self.inner
        //     .setup_build(
        //         source,
        //         subdirectory,
        //         install_path,
        //         version_id,
        //         dist,
        //         sources,
        //         build_kind,
        //         build_output,
        //     )
        //     .await
    }
}
