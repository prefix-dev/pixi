use std::hash::{Hash, Hasher};

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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnvironmentSpec {
    pub channels: Vec<ChannelUrl>,
    pub build_environment: BuildEnvironment,
    pub variants: VariantConfig,
    pub exclude_newer: Option<ResolvedExcludeNewer>,
    pub channel_priority: ChannelPriority,
}

// Manual `Hash` impl: `rattler_solve::ChannelPriority` doesn't implement
// `Hash`, so we fold it down to its discriminant. We destructure `Self`
// so that adding or renaming a field below triggers a compile error
// here, forcing a deliberate decision about its hash contribution.
// TODO: We can get rid of this once https://github.com/conda/rattler/pull/2373 is available.
impl Hash for EnvironmentSpec {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let Self {
            channels,
            build_environment,
            variants,
            exclude_newer,
            channel_priority,
        } = self;
        channels.hash(state);
        build_environment.hash(state);
        variants.hash(state);
        exclude_newer.hash(state);
        channel_priority_discriminant(channel_priority).hash(state);
    }
}

fn channel_priority_discriminant(priority: &ChannelPriority) -> u8 {
    match priority {
        ChannelPriority::Strict => 0,
        ChannelPriority::Disabled => 1,
    }
}
