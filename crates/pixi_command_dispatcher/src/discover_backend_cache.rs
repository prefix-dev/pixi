use std::{
    hash::Hash,
    path::{Path, PathBuf},
    sync::Arc,
};

use coalesced_map::{CoalescedGetError, CoalescedMap};
use pixi_build_discovery::{DiscoveredBackend, EnabledProtocols};
use rattler_conda_types::ChannelConfig;

use crate::CommandDispatcherError;

/// Keyed by canonicalized source path, enabled protocols, and channel config.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct DiscoveryKey {
    pub path: PathBuf,
    pub enabled_protocols: EnabledProtocols,
    pub channel_config: ChannelConfig,
}

/// Process-local cache that coalesces concurrent discovery requests and
/// memoizes results.
#[derive(Default, Clone)]
pub struct DiscoveryCache {
    inner: CoalescedMap<DiscoveryKey, Arc<DiscoveredBackend>>,
}

impl DiscoveryCache {
    /// Discover for a path, coalescing concurrent calls and caching the
    /// success.
    pub async fn get_or_discover(
        &self,
        source_path: &Path,
        channel_config: &ChannelConfig,
        enabled_protocols: &EnabledProtocols,
    ) -> Result<Arc<DiscoveredBackend>, CommandDispatcherError<pixi_build_discovery::DiscoveryError>>
    {
        let key = DiscoveryKey {
            path: dunce::canonicalize(source_path).unwrap_or_else(|_| source_path.to_path_buf()),
            enabled_protocols: enabled_protocols.clone(),
            channel_config: channel_config.clone(),
        };

        match self
            .inner
            .get_or_try_init(key, || {
                let source_path = source_path.to_path_buf();
                let channel_config = channel_config.clone();
                let enabled_protocols = enabled_protocols.clone();
                async move {
                    // Perform discovery on a blocking thread since it touches the filesystem.
                    let result = tokio::task::spawn_blocking(move || {
                        DiscoveredBackend::discover(
                            &source_path,
                            &channel_config,
                            &enabled_protocols,
                        )
                    })
                    .await
                    .map_err(|e| e.try_into_panic());

                    match result {
                        Ok(Ok(v)) => Ok(Arc::new(v)),
                        Ok(Err(e)) => Err(CommandDispatcherError::Failed(e)),
                        Err(Err(_)) => Err(CommandDispatcherError::Cancelled),
                        Err(Ok(panic)) => {
                            std::panic::resume_unwind(panic);
                        }
                    }
                }
            })
            .await
        {
            Ok(v) => Ok(v),
            Err(CoalescedGetError::Init(err)) => Err(err),
            Err(CoalescedGetError::CoalescedRequestFailed) => {
                Err(CommandDispatcherError::Cancelled)
            }
        }
    }
}
