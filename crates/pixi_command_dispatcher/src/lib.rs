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
//! The [`CommandDispatcher`] is built around a task-based execution model:
//!
//! 1. Each operation is represented as a task implementing the `TaskSpec` trait
//! 2. Tasks are submitted to a central queue and processed asynchronously
//! 3. Duplicate tasks are detected and consolidated to avoid redundant work
//! 4. Dependencies between tasks are tracked to ensure proper execution order
//! 5. Results are cached when appropriate to improve performance
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
mod cache_dirs;
mod command_dispatcher;
mod command_dispatcher_processor;
mod discover_backend_cache;
mod executor;
mod install_pixi;
mod instantiate_tool_env;
mod limits;
mod package_identifier;
pub mod reporter;
mod solve_conda;
mod solve_pixi;
mod source_build;
mod source_build_cache_status;
mod source_checkout;
mod source_metadata;

pub use backend_source_build::{
    BackendBuiltSource, BackendSourceBuildError, BackendSourceBuildMethod,
    BackendSourceBuildPrefix, BackendSourceBuildSpec, BackendSourceBuildV0Method,
    BackendSourceBuildV1Method,
};
pub use build::BuildEnvironment;
pub use build_backend_metadata::{
    BuildBackendMetadata, BuildBackendMetadataError, BuildBackendMetadataSpec,
};
pub use cache_dirs::CacheDirs;
pub use command_dispatcher::{
    CommandDispatcher, CommandDispatcherBuilder, CommandDispatcherError,
    CommandDispatcherErrorResultExt, InstantiateBackendError, InstantiateBackendSpec,
};
pub use executor::Executor;
pub use install_pixi::{
    InstallPixiEnvironmentError, InstallPixiEnvironmentResult, InstallPixiEnvironmentSpec,
};
pub use instantiate_tool_env::{InstantiateToolEnvironmentError, InstantiateToolEnvironmentSpec};
pub use limits::Limits;
pub use package_identifier::PackageIdentifier;
pub use reporter::{
    CondaSolveReporter, GitCheckoutReporter, PixiInstallReporter, PixiSolveReporter, Reporter,
    ReporterContext,
};
use serde::Serialize;
pub use solve_conda::SolveCondaEnvironmentSpec;
pub use solve_pixi::{PixiEnvironmentSpec, SolvePixiEnvironmentError};
pub use source_build::{SourceBuildError, SourceBuildResult, SourceBuildSpec};
pub use source_build_cache_status::{
    CachedBuildStatus, SourceBuildCacheEntry, SourceBuildCacheStatusError,
    SourceBuildCacheStatusSpec,
};
pub use source_checkout::{InvalidPathError, SourceCheckout, SourceCheckoutError};
pub use source_metadata::{Cycle, SourceMetadata, SourceMetadataError, SourceMetadataSpec};

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
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize)]
pub enum BuildProfile {
    /// Build a version of the package that is suitable for development.
    Development,

    /// Build a version of the package that is suitable for release.
    Release,
}
