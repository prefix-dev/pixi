//! Engine-wide configuration values fed to the compute engine as
//! [`InjectedKey`]s.
//!
//! Set once at dispatcher construction; any Key that needs one of these
//! reads it through the normal
//! [`ComputeCtx::compute`](pixi_compute_engine::ComputeCtx::compute)
//! call and records the dependency.

use std::sync::Arc;

use derive_more::Display;
use pixi_build_discovery::EnabledProtocols;
use pixi_build_frontend::BackendOverride;
use pixi_compute_engine::{BuildEnvironment, InjectedKey};
use rattler_conda_types::ChannelConfig;

/// Injected [`ChannelConfig`] for the dispatcher's engine.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("ChannelConfig")]
pub struct ChannelConfigKey;

impl InjectedKey for ChannelConfigKey {
    type Value = Arc<ChannelConfig>;
}

/// Injected [`EnabledProtocols`] for the dispatcher's engine.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("EnabledProtocols")]
pub struct EnabledProtocolsKey;

impl InjectedKey for EnabledProtocolsKey {
    type Value = Arc<EnabledProtocols>;
}

/// Injected build environment used for tool environments: the platform
/// and virtual packages derived from `tool_platform` at builder time.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("ToolBuildEnvironment")]
pub struct ToolBuildEnvironmentKey;

impl InjectedKey for ToolBuildEnvironmentKey {
    type Value = Arc<BuildEnvironment>;
}

/// Injected [`BackendOverride`] for the dispatcher's engine.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("BackendOverride")]
pub struct BackendOverrideKey;

impl InjectedKey for BackendOverrideKey {
    type Value = Arc<BackendOverride>;
}
