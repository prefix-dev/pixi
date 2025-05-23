//! A crate that is responsible for discovering the build backend that should
//! be used for a particular source tree.
//!
//!

mod backend_spec;
mod discovery;

pub use backend_spec::{
    BackendSpec, CommandSpec, EnvironmentSpec, JsonRpcBackendSpec, SystemCommandSpec,
};
pub use discovery::{
    BackendInitializationParams, DiscoveredBackend, DiscoveryError, EnabledProtocols,
};
