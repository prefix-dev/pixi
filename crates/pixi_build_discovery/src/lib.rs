//! A crate that is responsible for discovering the build backend that should
//! be used for a particular source tree.
//!
//! # Example
//!
//! ```rust,no_run
//! use pixi_build_discovery::{DiscoveredBackend, EnabledProtocols};
//! use rattler_conda_types::ChannelConfig;
//! use std::path::{Path, PathBuf};
//!
//! // Set up the channel configuration (typically from your environment)
//! let channel_config = ChannelConfig::default_with_root_dir(PathBuf::from("."));
//!
//! // Enable all protocols (default)
//! let enabled_protocols = EnabledProtocols::default();
//!
//! // Path to the source directory or manifest
//! let source_path = Path::new("/path/to/project");
//!
//! // Attempt to discover the backend
//! match DiscoveredBackend::discover(source_path, &channel_config, &enabled_protocols) {
//!     Ok(backend) => println!("Discovered backend: {:?}", backend.backend_spec),
//!     Err(e) => eprintln!("Failed to discover backend: {e}"),
//! }
//! ```
//!
//! This crate provides types and logic to identify and describe build backends for projects
//! using Pixi, Conda, or Rattler-based workflows. It supports discovery from both `pixi.toml`
//! and `recipe.yaml` manifests, and can be extended to support additional protocols.

mod backend_spec;
mod discovery;

pub use backend_spec::{
    BackendSpec, CommandSpec, EnvironmentSpec, JsonRpcBackendSpec, SystemCommandSpec,
};
pub use discovery::{
    BackendInitializationParams, DiscoveredBackend, DiscoveryError, EnabledProtocols,
};
