//! This crate provides a [`CommandDispatcher`]. The command dispatcher allows
//! constructing a graph of interdependent operations that can be executed
//! concurrently.
//!
//! # Overview
//!
//! For example, solving a pixi environment can entail many different recursive
//! tasks. When source dependencies are part of the environment, we need to
//! check out the source, construct another environment for the build backend,
//! get the metadata from the sourcecode and recursively iterate over any source
//! dependencies returned by the metadata. Pixi also not only does this for one
//! environment, but it needs to do this for multiple environments concurrently.
//!
//! We want to do this as efficiently as possible without recomputing the same
//! information or checking out sources twice. This requires some orchestration.
//! A [`CommandDispatcher`] is a tool that allows doing this.
//!
//! # Architecture
//!
//! The [`CommandDispatcher`] is a thin handle over a generic incremental
//! computation engine:
//!
//! 1. Each operation is a [`pixi_compute_engine::Key`] whose `compute`
//!    method produces its `Value`.
//! 2. Keys run through a [`pixi_compute_engine::ComputeEngine`], which
//!    dedups concurrent requests for the same key and caches results.
//! 3. Dependencies between keys are tracked implicitly via
//!    [`ComputeCtx::compute`](pixi_compute_engine::ComputeCtx::compute), and
//!    cycles are detected by the engine.
//! 4. Shared resources (gateway, caches, package cache, git/url resolvers,
//!    etc.) live in a typed [`DataStore`](pixi_compute_engine::DataStore)
//!    that keys read from via extension traits.
//!
//! # Usage
//!
//! The API of the [`CommandDispatcher`] is designed to be used in a way that
//! allows executing a request and awaiting the result. Multiple futures can be
//! created and awaited concurrently. This enables parallel execution while
//! maintaining a simple API surface.

mod backend_source_build;
pub mod build;
mod build_backend_metadata;
pub mod cache;
mod command_dispatcher;
pub mod compute_data;
mod cycle;
mod dev_source_metadata;
mod discovered_backend;
pub mod environment;
mod ephemeral_env;
mod errors;
mod injected_config;
mod input_hash;
mod install_binary;
mod install_pixi;
mod installed_source_hints;
mod instantiate_backend_key;
mod instantiate_tool_env;
pub mod keys;
pub mod reporter;
mod resolved_backend_command;
mod solve_binary;
mod solve_conda;
mod util;

pub use backend_source_build::{
    BackendBuiltSource, BackendSourceBuildError, BackendSourceBuildExt, BackendSourceBuildMethod,
    BackendSourceBuildPrefix, BackendSourceBuildSpec, BackendSourceBuildV1Method,
};
pub use build::BuildEnvironment;
pub use build_backend_metadata::{
    BuildBackendMetadata, BuildBackendMetadataError, BuildBackendMetadataInner,
    BuildBackendMetadataKey, BuildBackendMetadataSpec,
};
pub use cache::{
    BuildBackendMetadataCache, BuildBackendMetadataCacheEntry, CacheDirs, CacheEntry,
    CacheRevision, MetadataCache,
    markers::{
        BackendMetadataDir, BuildBackendsDir, LegacySourceEnvDir, PackagesDir,
        SourceBuildArtifactsDir, SourceBuildWorkspacesDir,
    },
};
pub use command_dispatcher::{
    CommandDispatcher, CommandDispatcherBuilder, CommandDispatcherError,
    CommandDispatcherErrorResultExt, ComputeResultExt,
};
pub use cycle::{Cycle, CycleEnvironment};
pub use dev_source_metadata::{
    DevSourceMetadata, DevSourceMetadataError, DevSourceMetadataKey, DevSourceMetadataSpec,
    PackageNotProvidedError,
};
pub use discovered_backend::DiscoveredBackendKey;
pub use environment::{
    BuildEnvOf, ChannelsOf, DerivedEnvKind, DerivedParent, EnvironmentRef, EnvironmentSpec,
    EphemeralEnv, ExcludeNewerOf, HasWorkspaceEnvRegistry, VariantsOf, WorkspaceEnvId,
    WorkspaceEnvRef, WorkspaceEnvRegistry,
};
pub use ephemeral_env::{
    EphemeralEnvError, EphemeralEnvKey, EphemeralEnvSpec, InstalledEphemeralEnv,
};
pub use errors::{
    MissingChannelError, SolvePixiEnvironmentError, SourceBuildError, SourceMetadataError,
    SourceRecordError,
};
pub use injected_config::{
    BackendOverrideKey, ChannelConfigKey, EnabledProtocolsKey, ToolBuildEnvironmentKey,
};
pub use install_pixi::{
    EnvironmentFingerprint, InstallPixiEnvironmentError, InstallPixiEnvironmentExt,
    InstallPixiEnvironmentResult, InstallPixiEnvironmentSpec,
};
pub use installed_source_hints::{InstalledSourceHint, InstalledSourceHints};
pub use instantiate_backend_key::{
    BackendHandle, InstantiateBackendError, InstantiateBackendKey, ProjectModelOverrides,
    resolve_backend_identifier,
};
pub use instantiate_tool_env::{InstantiateToolEnvironmentError, InstantiateToolEnvironmentSpec};
pub use keys::SourceMetadata;
pub use pixi_compute_cache_dirs::{
    CacheBase, CacheDirKey, CacheDirsExt, CacheDirsKey, CacheLocation,
};
pub use pixi_compute_env_vars::{EnvVar, EnvVarsKey};
pub use pixi_compute_sources::{
    GitCheckoutReporter, GitDir, InvalidPathError, SourceCheckout, SourceCheckoutError,
    SourceCheckoutExt, UrlCheckoutReporter, UrlDir,
};
pub use reporter::{
    BackendSourceBuildReporter, BuildBackendMetadataReporter, CondaSolveReporter, GatewayReporter,
    InstantiateBackendReporter, PixiInstallReporter, PixiSolveEnvironmentSpec, PixiSolveReporter,
    SourceMetadataReporter, SourceMetadataReporterSpec, SourceRecordReporter,
    SourceRecordReporterSpec,
};
pub use resolved_backend_command::{ResolvedBackendCommand, ResolvedBackendCommandKey};
use serde::Serialize;
pub use solve_conda::SolveCondaEnvironmentSpec;
pub use util::executor;
pub use util::{Executor, Limit, Limits, PtrArc};

// Re-export pixi_compute_engine types used by downstream crates.
pub use pixi_compute_engine::{ComputeCtx, ComputeEngine, ComputeError, Key};

/// A helper function to check if a value is the default value for its type.
fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    T::default() == *value
}

/// A build profile indicates the type of build that should happen. Dependencies
/// should not change regarding of the build profile, but the way the build is
/// executed can change. For example, a release build might use optimizations
/// while a development build might not.
///
/// ### Note
///
/// This feature is still in very early stages and is not yet fully implemented.
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq, Serialize)]
pub enum BuildProfile {
    /// Build a version of the package that is suitable for development.
    Development,

    /// Build a version of the package that is suitable for release.
    Release,
}
