//! Compute-engine Key that spawns a build-backend handle for a source
//! checkout. Discovers the backend via [`DiscoveredBackendKey`], applies
//! [`BackendOverrideKey`](crate::BackendOverrideKey) through
//! [`ResolvedBackendCommandKey`], then dispatches to an in-memory
//! instantiator, a system command, or a solved ephemeral build env. Keyed
//! on (source path, manifest anchor, build source dir, exclude_newer).

use std::{fmt, hash::Hash, path::PathBuf, sync::Arc};

use miette::Diagnostic;
use pixi_build_discovery::{
    BackendInitializationParams, BackendSpec, CommandSpec, DiscoveryError, EnvironmentSpec,
    JsonRpcBackendSpec, SystemCommandSpec,
};
use pixi_build_frontend::{
    Backend,
    in_memory::BoxedInMemoryBackend,
    json_rpc,
    json_rpc::{CommunicationError, JsonRpcBackend},
    tool::{IsolatedTool, SystemTool, Tool},
};
use pixi_build_types::{
    PIXI_BUILD_API_VERSION_NAME, PIXI_BUILD_API_VERSION_SPEC, PixiBuildApiVersion,
    procedures::initialize::InitializeParams,
};
use pixi_compute_engine::{ComputeCtx, Key};
use pixi_path::AbsPresumedDirPathBuf;
use pixi_record::PixiRecord;
use pixi_spec::{BinarySpec, ResolvedExcludeNewer, SourceAnchor, SpecConversionError};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{PackageName, VersionWithSource};
use rattler_shell::{
    activation::{ActivationError, ActivationVariables, Activator},
    shell::ShellEnum,
};
use thiserror::Error;
use tokio::sync::Mutex;

use crate::compute_data::{HasCacheDirs, HasReporter};
use crate::discovered_backend::DiscoveredBackendKey;
use crate::ephemeral_env::{EphemeralEnvError, EphemeralEnvKey, EphemeralEnvSpec};
use crate::injected_config::ToolBuildEnvironmentKey;
use crate::reporter::{
    InstantiateBackendId, InstantiateBackendReporter, Reporter, ReporterContext,
};
use crate::reporter_context::{CURRENT_REPORTER_CONTEXT, current_reporter_context};
use crate::reporter_lifecycle::{Active, LifecycleKind, ReporterLifecycle};
use crate::resolved_backend_command::{ResolvedBackendCommand, ResolvedBackendCommandKey};

/// Dedup key for spawning a build backend.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct InstantiateBackendKey {
    /// Path to the directory containing the manifest. Canonicalized at
    /// construction.
    pub source_path: PathBuf,

    /// Anchor used to resolve relative source refs in the discovered
    /// backend's `BackendSpec`.
    pub manifest_source_anchor: SourceAnchor,

    /// Directory to build from. May differ from `source_path` for
    /// out-of-tree builds.
    pub build_source_dir: AbsPresumedDirPathBuf,

    /// Exclude-newer cutoff applied when solving the backend's tool env.
    pub exclude_newer: Option<ResolvedExcludeNewer>,
}

impl InstantiateBackendKey {
    pub fn new(
        source_path: impl AsRef<std::path::Path>,
        manifest_source_anchor: SourceAnchor,
        build_source_dir: AbsPresumedDirPathBuf,
        exclude_newer: Option<ResolvedExcludeNewer>,
    ) -> Self {
        let source_path = source_path.as_ref();
        let canonical =
            dunce::canonicalize(source_path).unwrap_or_else(|_| source_path.to_path_buf());
        Self {
            source_path: canonical,
            manifest_source_anchor,
            build_source_dir,
            exclude_newer,
        }
    }
}

impl fmt::Display for InstantiateBackendKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "backend@{}", self.source_path.display())
    }
}

/// Errors from [`InstantiateBackendKey::compute`].
#[derive(Debug, Clone, Error, Diagnostic)]
pub enum InstantiateBackendError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Discovery(Arc<DiscoveryError>),

    #[error(transparent)]
    #[diagnostic(transparent)]
    EphemeralEnv(#[from] Arc<EphemeralEnvError>),

    #[error(transparent)]
    #[diagnostic(transparent)]
    JsonRpc(Arc<json_rpc::InitializeError>),

    #[error(transparent)]
    #[diagnostic(transparent)]
    InMemory(Arc<CommunicationError>),

    #[error("failed to run activation for the backend tool")]
    Activation(Arc<ActivationError>),

    #[error(transparent)]
    SpecConversion(Arc<SpecConversionError>),

    #[error(
        "the environment for build backend `{}` does not depend on `{}`; pixi cannot negotiate an API version",
        .build_backend_name,
        PIXI_BUILD_API_VERSION_NAME.as_normalized()
    )]
    NoMatchingApiVersion { build_backend_name: String },

    #[error("failed to canonicalize source directory `{0}`")]
    Canonicalize(PathBuf, #[source] Arc<std::io::Error>),
}

/// Handle to a spawned backend. Wrapped in a [`Mutex`] because the
/// JSON-RPC transport serializes request/response over one stdio pipe
/// and the `InMemoryBackend` trait object is not `Sync`.
pub type BackendHandle = Arc<Mutex<Backend>>;

impl Key for InstantiateBackendKey {
    type Value = Result<BackendHandle, Arc<InstantiateBackendError>>;

    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        // Discover the backend for this source path. Discovery runs
        // before the reporter lifecycle is queued so the `on_queued`
        // event can carry the resolved backend name.
        let discovered = ctx
            .compute(&DiscoveredBackendKey::new(&self.source_path))
            .await
            .map_err(|e| Arc::new(InstantiateBackendError::Discovery(e)))?;

        // Resolve the backend spec with the manifest anchor.
        let BackendSpec::JsonRpc(resolved_spec) = discovered
            .backend_spec
            .clone()
            .resolve(self.manifest_source_anchor.clone());

        // Fire the InstantiateBackendReporter lifecycle and scope
        // CURRENT_REPORTER_CONTEXT so nested tasks (override resolve,
        // ephemeral env solve/install, JSON-RPC setup) attribute their
        // progress to this backend instantiation.
        let reporter_arc = ctx.global_data().reporter().cloned();
        let lifecycle = ReporterLifecycle::<InstantiateBackendLifecycle>::queued(
            reporter_arc.as_deref(),
            current_reporter_context(),
            &resolved_spec,
        );
        let reporter_ctx = lifecycle.id().map(ReporterContext::InstantiateBackend);
        let _started = lifecycle.start();

        let work = self.compute_inner(ctx, discovered, resolved_spec);
        match reporter_ctx {
            Some(rc) => CURRENT_REPORTER_CONTEXT.scope(Some(rc), work).await,
            None => work.await,
        }
    }
}

/// Reporter lifecycle bridging [`InstantiateBackendReporter`] into the
/// generic [`ReporterLifecycle`] typestate.
struct InstantiateBackendLifecycle;

impl LifecycleKind for InstantiateBackendLifecycle {
    type Reporter<'r> = dyn InstantiateBackendReporter + 'r;
    type Id = InstantiateBackendId;
    type Env = pixi_build_discovery::JsonRpcBackendSpec;

    fn queue<'r>(
        reporter: Option<&'r dyn Reporter>,
        parent: Option<ReporterContext>,
        env: &Self::Env,
    ) -> Option<Active<'r, Self::Reporter<'r>, Self::Id>> {
        reporter
            .and_then(|r| r.as_instantiate_backend_reporter())
            .map(|r| Active {
                reporter: r,
                id: r.on_queued(parent, env),
            })
    }

    fn on_started<'r>(active: &Active<'r, Self::Reporter<'r>, Self::Id>) {
        active.reporter.on_started(active.id);
    }

    fn on_finished<'r>(active: Active<'r, Self::Reporter<'r>, Self::Id>) {
        active.reporter.on_finished(active.id);
    }
}

impl InstantiateBackendKey {
    /// The main body of [`Key::compute`], split out so the outer
    /// `compute` can keep the reporter lifecycle + context scope
    /// tightly wrapped around the long-running work.
    async fn compute_inner(
        &self,
        ctx: &mut ComputeCtx,
        discovered: Arc<pixi_build_discovery::DiscoveredBackend>,
        resolved_spec: JsonRpcBackendSpec,
    ) -> Result<BackendHandle, Arc<InstantiateBackendError>> {
        let source_dir = self.canonical_build_source_dir()?;

        // Apply the engine's backend override to the resolved spec.
        let resolved_command = ctx
            .compute(&ResolvedBackendCommandKey::new(resolved_spec.clone()))
            .await;

        // Build the tool. InMemory short-circuits to a fully-initialized
        // backend because `.initialize(..)` needs per-request params.
        let (tool, api_version) = match resolved_command.as_ref() {
            ResolvedBackendCommand::InMemory(in_mem) => {
                return self.instantiate_in_memory(
                    ctx,
                    in_mem,
                    &source_dir,
                    &discovered.init_params,
                );
            }
            ResolvedBackendCommand::Spec(CommandSpec::System(system_spec)) => (
                tool_for_system_spec(system_spec, &resolved_spec.name),
                PixiBuildApiVersion::current(),
            ),
            ResolvedBackendCommand::Spec(CommandSpec::EnvironmentSpec(env_spec)) => {
                self.tool_for_environment_spec(ctx, &resolved_spec, env_spec)
                    .await?
            }
        };

        tracing::info!(
            "Instantiated backend {}{}, negotiated API version {}{}",
            tool.executable(),
            tool.version().map_or_else(String::new, |v| format!("@{v}")),
            api_version,
            if let Some(isolated) = tool.as_isolated() {
                format!(", from prefix {}", isolated.prefix().display())
            } else {
                String::new()
            },
        );

        check_project_model_invariant(api_version, &discovered.init_params)?;

        spawn_json_rpc(ctx, source_dir, &discovered.init_params, tool, api_version).await
    }

    /// Canonicalize the build source directory.
    fn canonical_build_source_dir(&self) -> Result<PathBuf, Arc<InstantiateBackendError>> {
        dunce::canonicalize(self.build_source_dir.as_std_path()).map_err(|e| {
            Arc::new(InstantiateBackendError::Canonicalize(
                self.build_source_dir.as_std_path().to_path_buf(),
                Arc::new(e),
            ))
        })
    }

    /// Short-circuit: drive the in-memory backend factory with the
    /// per-request init params and wrap the resulting [`Backend`].
    fn instantiate_in_memory(
        &self,
        ctx: &ComputeCtx,
        in_mem: &BoxedInMemoryBackend,
        source_dir: &std::path::Path,
        init_params: &BackendInitializationParams,
    ) -> Result<BackendHandle, Arc<InstantiateBackendError>> {
        let memory = in_mem
            .initialize(InitializeParams {
                manifest_path: init_params.manifest_path.clone(),
                source_directory: Some(source_dir.to_path_buf()),
                workspace_directory: Some(init_params.workspace_root.clone()),
                cache_directory: Some(ctx.global_data().cache_dirs().root().to_owned().into()),
                project_model: init_params.project_model.clone(),
                configuration: init_params.configuration.clone(),
                target_configuration: init_params.target_configuration.clone(),
            })
            .map_err(|e| Arc::new(InstantiateBackendError::InMemory(Arc::new(*e))))?;
        Ok(Arc::new(Mutex::new(Backend::new(
            memory.into(),
            in_mem.api_version(),
        ))))
    }

    /// Construct an [`IsolatedTool`] by computing the ephemeral env for
    /// the backend's [`EnvironmentSpec`], extracting the negotiated API
    /// version + primary package version from the install, and running
    /// activation against the prefix.
    async fn tool_for_environment_spec(
        &self,
        ctx: &mut ComputeCtx,
        resolved_spec: &JsonRpcBackendSpec,
        env_spec: &EnvironmentSpec,
    ) -> Result<(Tool, PixiBuildApiVersion), Arc<InstantiateBackendError>> {
        let ephemeral_spec = self.ephemeral_env_spec_for(env_spec);
        let installed = ctx
            .compute(&EphemeralEnvKey::new(ephemeral_spec))
            .await
            .map_err(InstantiateBackendError::EphemeralEnv)?;

        let api = api_version_from_records(&installed.records).ok_or_else(|| {
            Arc::new(InstantiateBackendError::NoMatchingApiVersion {
                build_backend_name: resolved_spec.name.clone(),
            })
        })?;
        let version =
            primary_package_version_from_records(&installed.records, &env_spec.requirement.0)
                .expect("solved env contains the requested primary package");

        let host_platform = ctx.compute(&ToolBuildEnvironmentKey).await.host_platform;
        let activator =
            Activator::from_path(installed.prefix.path(), ShellEnum::default(), host_platform)
                .map_err(|e| Arc::new(InstantiateBackendError::Activation(Arc::new(e))))?;
        let activation_scripts = activator
            .run_activation(ActivationVariables::from_env().unwrap_or_default(), None)
            .map_err(|e| Arc::new(InstantiateBackendError::Activation(Arc::new(e))))?;

        let tool = Tool::from(IsolatedTool::new(
            env_spec
                .command
                .clone()
                .unwrap_or_else(|| resolved_spec.name.clone()),
            Some(version),
            installed.prefix.path().to_path_buf(),
            activation_scripts,
        ));
        Ok((tool, api))
    }

    /// Build an [`EphemeralEnvSpec`] from the backend's
    /// [`EnvironmentSpec`]: merge `requirement` into `dependencies`,
    /// append the `PIXI_BUILD_API_VERSION` constraint so the solved
    /// env surfaces the negotiated API version, and carry through the
    /// key's exclude-newer cutoff.
    fn ephemeral_env_spec_for(&self, env_spec: &EnvironmentSpec) -> EphemeralEnvSpec {
        let mut dependencies: DependencyMap<PackageName, pixi_spec::PixiSpec> =
            env_spec.additional_requirements.clone();
        dependencies.insert(
            env_spec.requirement.0.clone(),
            env_spec.requirement.1.clone(),
        );

        let mut constraints = env_spec.constraints.clone();
        constraints.insert(
            PIXI_BUILD_API_VERSION_NAME.clone(),
            BinarySpec::Version(PIXI_BUILD_API_VERSION_SPEC.clone()),
        );

        EphemeralEnvSpec {
            dependencies,
            constraints,
            channels: env_spec.channels.clone(),
            exclude_newer: self.exclude_newer.clone(),
            strategy: Default::default(),
            channel_priority: Default::default(),
        }
    }
}

/// Build a [`Tool::System`] that runs the backend's executable directly
/// (either as specified by the `System` override or the backend name).
fn tool_for_system_spec(system_spec: &SystemCommandSpec, default_name: &str) -> Tool {
    Tool::System(SystemTool::new(
        system_spec
            .command
            .clone()
            .unwrap_or_else(|| default_name.to_string()),
    ))
}

/// Find the negotiated [`PixiBuildApiVersion`] in the solved records
/// by looking up the `pixi-build-api-version` package.
fn api_version_from_records(records: &[PixiRecord]) -> Option<PixiBuildApiVersion> {
    records.iter().find_map(|r| match r {
        PixiRecord::Binary(b) if b.package_record.name == *PIXI_BUILD_API_VERSION_NAME => {
            PixiBuildApiVersion::from_version(b.package_record.version.as_ref())
        }
        _ => None,
    })
}

/// Find the version of the primary backend package in the solved records.
fn primary_package_version_from_records(
    records: &[PixiRecord],
    name: &PackageName,
) -> Option<VersionWithSource> {
    records.iter().find_map(|r| match r {
        PixiRecord::Binary(b) if b.package_record.name == *name => {
            Some(b.package_record.version.clone())
        }
        _ => None,
    })
}

/// If the negotiated API version requires a named project model but
/// discovery produced a model without a name, fail up front with a
/// clearer error than the JSON-RPC handshake would give.
fn check_project_model_invariant(
    api_version: PixiBuildApiVersion,
    init_params: &BackendInitializationParams,
) -> Result<(), Arc<InstantiateBackendError>> {
    if !api_version.supports_name_none()
        && init_params
            .project_model
            .as_ref()
            .is_some_and(|p| p.name.is_none())
    {
        return Err(Arc::new(InstantiateBackendError::SpecConversion(Arc::new(
            SpecConversionError::MissingName,
        ))));
    }
    Ok(())
}

/// Spawn a JSON-RPC backend for `tool` and wrap it in the standard
/// [`BackendHandle`] mutex.
async fn spawn_json_rpc(
    ctx: &ComputeCtx,
    source_dir: PathBuf,
    init_params: &BackendInitializationParams,
    tool: Tool,
    api_version: PixiBuildApiVersion,
) -> Result<BackendHandle, Arc<InstantiateBackendError>> {
    let cache_dir = ctx.global_data().cache_dirs().root().to_owned().into();
    let backend = JsonRpcBackend::setup(
        source_dir,
        init_params.manifest_path.clone(),
        init_params.workspace_root.clone(),
        init_params.project_model.clone(),
        init_params.configuration.clone(),
        init_params.target_configuration.clone(),
        Some(cache_dir),
        tool,
    )
    .await
    .map_err(|e| Arc::new(InstantiateBackendError::JsonRpc(Arc::new(e))))?;
    Ok(Arc::new(Mutex::new(Backend::new(
        backend.into(),
        api_version,
    ))))
}
