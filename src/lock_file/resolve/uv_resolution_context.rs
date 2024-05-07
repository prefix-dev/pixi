use std::sync::Arc;

use miette::{Context, IntoDiagnostic};
use uv_cache::Cache;
use uv_configuration::{NoBinary, NoBuild};
use uv_types::{HashStrategy, InFlight};

use crate::{
    config::{self, get_cache_dir},
    Project,
};

/// Objects that are needed for resolutions which can be shared between different resolutions.
#[derive(Clone)]
pub struct UvResolutionContext {
    pub cache: Cache,
    pub in_flight: Arc<InFlight>,
    pub no_build: NoBuild,
    pub no_binary: NoBinary,
    pub hash_strategy: HashStrategy,
    pub client: reqwest::Client,
    pub keyring_provider: uv_configuration::KeyringProviderType,
}

impl UvResolutionContext {
    pub fn from_project(project: &Project) -> miette::Result<Self> {
        let cache = Cache::from_path(
            get_cache_dir()
                .expect("missing caching directory")
                .join("uv-cache"),
        )
        .into_diagnostic()
        .context("failed to create uv cache")?;

        let keyring_provider = match project.config().pypi_config().use_keyring() {
            config::KeyringProvider::Subprocess => {
                tracing::info!("using uv keyring (subprocess) provider");
                uv_configuration::KeyringProviderType::Subprocess
            }
            config::KeyringProvider::Disabled => {
                tracing::info!("uv keyring provider is disabled");
                uv_configuration::KeyringProviderType::Disabled
            }
        };

        let in_flight = Arc::new(InFlight::default());
        Ok(Self {
            cache,
            in_flight,
            no_build: NoBuild::None,
            no_binary: NoBinary::None,
            hash_strategy: HashStrategy::None,
            client: project.client().clone(),
            keyring_provider,
        })
    }
}
