use pixi_spec::ResolvedExcludeNewer;
use pixi_utils::variants::VariantConfig;
use rattler_conda_types::ChannelUrl;
use rattler_solve::ChannelPriority;

use crate::BuildEnvironment;

/// Snapshot of the environment-level inputs a solve or metadata fetch
/// depends on. Stored in [`WorkspaceEnvRegistry`](super::WorkspaceEnvRegistry)
/// under a [`WorkspaceEnvId`](super::WorkspaceEnvId); projections read
/// individual fields from it via
/// [`ComputeCtx::global_data`](pixi_compute_engine::ComputeCtx::global_data).
///
/// Identity is purely content-driven. There is no label field: name
/// and platform travel on [`WorkspaceEnvRef`](super::WorkspaceEnvRef)
/// for display only and are not part of identity. `installed` hints
/// are not carried here; they live on
/// [`SolvePixiEnvironmentSpec`](crate::keys::SolvePixiEnvironmentSpec)
/// because `PixiRecord` does not implement `Hash`.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EnvironmentSpec {
    pub channels: Vec<ChannelUrl>,
    pub build_environment: BuildEnvironment,
    pub variants: VariantConfig,
    pub exclude_newer: Option<ResolvedExcludeNewer>,
    pub channel_priority: ChannelPriority,
}
