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
//! 1. Each operation is represented as a task implementing the `TaskSpec`
//!    trait
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

mod build;
mod cache_dirs;
mod command_dispatcher;
mod command_dispatcher_processor;
mod executor;
mod install_pixi;
mod instantiate_tool_env;
mod limits;
pub mod reporter;
mod solve_conda;
mod solve_pixi;
mod source_checkout;
mod source_metadata;

pub use build::BuildEnvironment;
pub use cache_dirs::CacheDirs;
pub use command_dispatcher::{
    CommandDispatcher, CommandDispatcherBuilder, CommandDispatcherError,
    CommandDispatcherErrorResultExt, InstantiateBackendError, InstantiateBackendSpec,
};
pub use executor::Executor;
pub use install_pixi::{InstallPixiEnvironmentError, InstallPixiEnvironmentSpec};
pub use limits::Limits;
pub use reporter::{
    CondaSolveReporter, GitCheckoutReporter, PixiInstallReporter, PixiSolveReporter, Reporter,
    ReporterContext,
};
pub use solve_conda::SolveCondaEnvironmentSpec;
pub use solve_pixi::{PixiEnvironmentSpec, SolvePixiEnvironmentError};
pub use source_checkout::{InvalidPathError, SourceCheckout, SourceCheckoutError};
pub use source_metadata::SourceMetadataSpec;

/// A helper function to check if a value is the default value for its type.
fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    T::default() == *value
}

#[cfg(test)]
mod test {}
