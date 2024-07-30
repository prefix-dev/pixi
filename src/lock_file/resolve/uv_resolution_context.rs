use std::sync::Arc;

use miette::{Context, IntoDiagnostic};
use uv_cache::Cache;
use uv_configuration::{BuildOptions, Concurrency};
use uv_types::{HashStrategy, InFlight};

use crate::Project;
use pixi_config::{self, get_cache_dir};
use pixi_consts::consts;

/// Objects that are needed for resolutions which can be shared between different resolutions.
#[derive(Clone)]
pub struct UvResolutionContext {
    pub cache: Cache,
    pub in_flight: Arc<InFlight>,
    pub build_options: BuildOptions,
    pub hash_strategy: HashStrategy,
    pub client: reqwest::Client,
    pub keyring_provider: uv_configuration::KeyringProviderType,
    pub concurrency: Concurrency,
}

impl UvResolutionContext {
    pub fn from_project(project: &Project) -> miette::Result<Self> {
        let uv_cache = get_cache_dir()?.join(consts::PYPI_CACHE_DIR);
        if !uv_cache.exists() {
            std::fs::create_dir_all(&uv_cache)
                .into_diagnostic()
                .context("failed to create uv cache directory")?;
        }

        let cache = Cache::from_path(uv_cache);

        let keyring_provider = match project.config().pypi_config().use_keyring() {
            pixi_config::KeyringProvider::Subprocess => {
                tracing::info!("using uv keyring (subprocess) provider");
                uv_configuration::KeyringProviderType::Subprocess
            }
            pixi_config::KeyringProvider::Disabled => {
                tracing::info!("uv keyring provider is disabled");
                uv_configuration::KeyringProviderType::Disabled
            }
        };

        let in_flight = Arc::new(InFlight::default());
        Ok(Self {
            cache,
            in_flight,
            hash_strategy: HashStrategy::None,
            client: project.client().clone(),
            build_options: BuildOptions::default(),
            keyring_provider,
            concurrency: Concurrency::default(),
        })
    }
}
