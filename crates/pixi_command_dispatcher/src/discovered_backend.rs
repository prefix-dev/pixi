//! Compute-engine Key for discovering the build backend of a source
//! checkout.
//!
//! [`DiscoveredBackendKey`] replaces the previous `DiscoveryCache`: it
//! keys on the already-checked-out source path and runs
//! [`DiscoveredBackend::discover`] on a blocking thread. The channel
//! configuration and enabled protocols come from injected values
//! (see [`crate::injected_config`]) so engine-wide settings apply
//! uniformly without bloating the key identity.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use derive_more::Display;
use pixi_build_discovery::{DiscoveredBackend, DiscoveryError};
use pixi_compute_engine::{ComputeCtx, Key};

use crate::injected_config::{ChannelConfigKey, EnabledProtocolsKey};

/// Discover the build backend for a checked-out source path.
///
/// The inner path is canonicalized at construction so two callers that
/// name the same directory via different casing or through symlinks
/// dedup on the same key. Construct via [`DiscoveredBackendKey::new`].
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{}", _0.display())]
pub struct DiscoveredBackendKey(PathBuf);

impl DiscoveredBackendKey {
    pub fn new(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let canonical = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        Self(canonical)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Key for DiscoveredBackendKey {
    type Value = Result<Arc<DiscoveredBackend>, Arc<DiscoveryError>>;

    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let channel_config = ctx.compute(&ChannelConfigKey).await;
        let enabled_protocols = ctx.compute(&EnabledProtocolsKey).await;
        let path = self.0.clone();

        tokio::task::spawn_blocking(move || {
            DiscoveredBackend::discover(&path, &channel_config, &enabled_protocols)
        })
        .await
        .unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()))
        .map(Arc::new)
        .map_err(Arc::new)
    }
}
