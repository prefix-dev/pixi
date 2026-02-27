use std::sync::Arc;

use fs_err::create_dir_all;
use miette::{Context, IntoDiagnostic};
use pixi_config::{self, Config, get_cache_dir};
use pixi_consts::consts;
use pixi_utils::reqwest::{
    should_use_builtin_certs_uv, should_use_native_tls_for_uv, uv_middlewares,
};
use pixi_uv_conversions::{ConversionError, to_uv_trusted_host};
use tracing::debug;
use uv_cache::Cache;
use uv_client::{
    BaseClientBuilder, Connectivity, ExtraMiddleware, RegistryClient, RegistryClientBuilder,
};
use uv_configuration::{Concurrency, IndexStrategy, SourceStrategy, TrustedHost};
use uv_dispatch::SharedState;
use uv_distribution_types::{
    ExtraBuildRequires, ExtraBuildVariables, IndexCapabilities, IndexLocations,
    PackageConfigSettings,
};
use uv_pep508::MarkerEnvironment;
use uv_preview::Preview;
use uv_types::{HashStrategy, InFlight};
use uv_workspace::WorkspaceCache;

/// Objects that are needed for resolutions which can be shared between different resolutions.
#[derive(Clone)]
pub struct UvResolutionContext {
    pub cache: Cache,
    pub in_flight: InFlight,
    pub hash_strategy: HashStrategy,
    pub keyring_provider: uv_configuration::KeyringProviderType,
    pub concurrency: Concurrency,
    pub source_strategy: SourceStrategy,
    pub capabilities: IndexCapabilities,
    pub allow_insecure_host: Vec<TrustedHost>,
    pub shared_state: SharedState,
    pub extra_middleware: ExtraMiddleware,
    pub proxies: Vec<reqwest::Proxy>,
    pub tls_no_verify: bool,
    /// Whether UV should use native TLS (system certificates).
    /// This is computed based on the `tls-root-certs` config and the TLS feature used.
    pub use_native_tls: bool,
    pub use_builtin_certs: bool,
    pub package_config_settings: PackageConfigSettings,
    pub extra_build_requires: ExtraBuildRequires,
    pub extra_build_variables: ExtraBuildVariables,
    pub preview: Preview,
    pub workspace_cache: WorkspaceCache,
}

impl UvResolutionContext {
    pub fn from_config(config: &Config) -> miette::Result<Self> {
        let uv_cache = get_cache_dir()?.join(consts::PYPI_CACHE_DIR);
        if !uv_cache.exists() {
            create_dir_all(&uv_cache)
                .into_diagnostic()
                .context("failed to create uv cache directory")?;
        }

        let cache = Cache::from_path(uv_cache);

        let keyring_provider = match config.pypi_config.use_keyring() {
            pixi_config::KeyringProvider::Subprocess => {
                debug!("using uv keyring (subprocess) provider");
                uv_configuration::KeyringProviderType::Subprocess
            }
            pixi_config::KeyringProvider::Disabled => {
                debug!("uv keyring provider is disabled");
                uv_configuration::KeyringProviderType::Disabled
            }
        };

        let allow_insecure_host = config
            .pypi_config
            .allow_insecure_host
            .iter()
            .try_fold(
                Vec::new(),
                |mut hosts, host| -> Result<Vec<TrustedHost>, ConversionError> {
                    let parsed = to_uv_trusted_host(host)?;
                    hosts.push(parsed);
                    Ok(hosts)
                },
            )
            .into_diagnostic()
            .context("failed to parse trusted host")?;
        Ok(Self {
            cache,
            in_flight: InFlight::default(),
            hash_strategy: HashStrategy::None,
            keyring_provider,
            concurrency: Concurrency::default(),
            source_strategy: SourceStrategy::Enabled,
            capabilities: IndexCapabilities::default(),
            allow_insecure_host,
            shared_state: SharedState::default(),
            extra_middleware: ExtraMiddleware(uv_middlewares(config)),
            proxies: config.get_proxies().into_diagnostic()?,
            tls_no_verify: config.tls_no_verify(),
            use_native_tls: should_use_native_tls_for_uv(),
            use_builtin_certs: should_use_builtin_certs_uv(config),
            package_config_settings: PackageConfigSettings::default(),
            extra_build_requires: ExtraBuildRequires::default(),
            extra_build_variables: ExtraBuildVariables::default(),
            preview: Preview::default(),
            workspace_cache: WorkspaceCache::default(),
        })
    }

    /// Set the cache refresh strategy.
    pub fn set_cache_refresh(
        mut self,
        all: Option<bool>,
        specific_packages: Option<Vec<uv_normalize::PackageName>>,
    ) -> Self {
        let policy = uv_cache::Refresh::from_args(all, specific_packages.unwrap_or_default());
        self.cache = self.cache.with_refresh(policy);
        self
    }

    /// Build a registry client configured with the context settings.
    ///
    /// Parameters:
    /// - `allow_insecure_hosts`: Pre-computed insecure hosts (use
    ///   `configure_insecure_hosts_for_tls_bypass`)
    /// - `index_locations`: The index locations to use
    /// - `index_strategy`: The index strategy to use
    /// - `markers`: Optional marker environment for platform-specific resolution
    /// - `connectivity`: Whether to allow network access
    pub fn build_registry_client(
        &self,
        allow_insecure_hosts: Vec<TrustedHost>,
        index_locations: &IndexLocations,
        index_strategy: IndexStrategy,
        markers: Option<&MarkerEnvironment>,
        connectivity: Connectivity,
    ) -> Arc<RegistryClient> {
        let mut base_client_builder = BaseClientBuilder::default()
            .allow_insecure_host(allow_insecure_hosts)
            .keyring(self.keyring_provider)
            .connectivity(connectivity)
            .native_tls(self.use_native_tls)
            .built_in_root_certs(self.use_builtin_certs)
            .extra_middleware(self.extra_middleware.clone());

        if let Some(markers) = markers {
            base_client_builder = base_client_builder.markers(markers);
        }

        let mut uv_client_builder =
            RegistryClientBuilder::new(base_client_builder, self.cache.clone())
                .index_locations(index_locations.clone())
                .index_strategy(index_strategy);

        for p in &self.proxies {
            uv_client_builder = uv_client_builder.proxy(p.clone());
        }

        Arc::new(uv_client_builder.build())
    }
}
