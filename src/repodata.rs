use rattler_repodata_gateway::{ChannelConfig, Gateway};
use std::path::PathBuf;

pub(crate) trait Repodata {
    /// Initialized the [`Gateway`]
    fn repodata_gateway_init(
        authenticated_client: reqwest_middleware::ClientWithMiddleware,
        channel_config: ChannelConfig,
    ) -> Gateway {
        // Determine the cache directory and fall back to sane defaults otherwise.
        let cache_dir = pixi_config::get_cache_dir().unwrap_or_else(|e| {
            tracing::error!("failed to determine repodata cache directory: {e}");
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("./"))
        });

        // Construct the gateway
        Gateway::builder()
            .with_client(authenticated_client)
            .with_cache_dir(cache_dir.join(pixi_consts::consts::CONDA_REPODATA_CACHE_DIR))
            .with_channel_config(channel_config)
            .finish()
    }

    /// Returns the [`Gateway`] used by this project.
    fn repodata_gateway(&self) -> &Gateway;
}
