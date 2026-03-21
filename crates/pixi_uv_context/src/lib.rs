use std::{path::Path, str::FromStr, sync::Arc};

use fs_err::create_dir_all;
use indexmap::IndexMap;
use miette::{Context, IntoDiagnostic};
use pixi_config::{self, Config, get_cache_dir};
use pixi_consts::consts;
use pixi_pypi_spec::PypiPackageName;
use pixi_utils::reqwest::{
    LazyReqwestClient, should_use_builtin_certs_uv, should_use_native_tls_for_uv, uv_middlewares,
};
use pixi_uv_conversions::{
    ConversionError, pep508_requirement_to_uv_requirement, to_uv_trusted_host,
};
use tracing::debug;
use uv_cache::Cache;
use uv_client::{
    BaseClientBuilder, Connectivity, ExtraMiddleware, RegistryClient, RegistryClientBuilder,
};
use uv_configuration::{Concurrency, IndexStrategy, SourceStrategy, TrustedHost};
use uv_dispatch::SharedState;
use uv_distribution_types::{
    ExtraBuildRequirement, ExtraBuildRequires, ExtraBuildVariables, IndexCapabilities,
    IndexLocations, PackageConfigSettings,
};
use uv_pep508::MarkerEnvironment;
use uv_preview::Preview;
use uv_types::{HashStrategy, InFlight};
use uv_workspace::WorkspaceCache;

/// Convert manifest-defined extra build dependencies into uv's
/// [`ExtraBuildRequires`] structure.
///
/// Each manifest requirement is converted to a uv requirement and wrapped in an
/// [`ExtraBuildRequirement`] with `match_runtime = false` for v1 behavior.
///
/// The `workspace_root` parameter is currently unused but kept in the API to
/// preserve call-site intent and future conversion support for path-based
/// requirement forms.
pub fn convert_extra_build_dependencies(
    deps: &Option<IndexMap<PypiPackageName, Vec<pep508_rs::Requirement>>>,
    _workspace_root: &Path,
) -> Result<ExtraBuildRequires, pixi_uv_conversions::ConversionError> {
    let mut extra_build_requires = ExtraBuildRequires::default();

    for (package, specs) in deps.iter().flatten() {
        let requirements = specs
            .iter()
            .map(|spec| {
                pep508_requirement_to_uv_requirement(spec.clone()).map(|requirement| {
                    ExtraBuildRequirement {
                        requirement,
                        match_runtime: false,
                    }
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        if requirements.is_empty() {
            continue;
        }

        let package_name = uv_normalize::PackageName::from_str(package.as_normalized().as_ref())
            .expect("pypi package names in manifest should always be valid");
        extra_build_requires.insert(package_name, requirements);
    }

    Ok(extra_build_requires)
}

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
    pub fn from_config(config: &Config, client: LazyReqwestClient) -> miette::Result<Self> {
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
            extra_middleware: ExtraMiddleware(uv_middlewares(config, client)),
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
    pub fn build_registry_client(
        &self,
        allow_insecure_hosts: Vec<TrustedHost>,
        index_locations: &IndexLocations,
        index_strategy: IndexStrategy,
        markers: Option<&MarkerEnvironment>,
    ) -> Arc<RegistryClient> {
        let mut base_client_builder = BaseClientBuilder::default()
            .allow_insecure_host(allow_insecure_hosts)
            .keyring(self.keyring_provider)
            .connectivity(Connectivity::Online)
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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use indexmap::IndexMap;
    use pixi_pypi_spec::PypiPackageName;

    use super::convert_extra_build_dependencies;

    #[test]
    fn converts_extra_build_dependencies() {
        let mut deps: IndexMap<PypiPackageName, Vec<pep508_rs::Requirement>> = IndexMap::new();
        deps.insert(
            PypiPackageName::from_str("fused-ssim").unwrap(),
            vec![pep508_rs::Requirement::from_str("torch>=2").unwrap()],
        );

        let converted = convert_extra_build_dependencies(&Some(deps), std::path::Path::new("."))
            .expect("conversion should succeed");

        let pkg_name = uv_normalize::PackageName::from_str("fused-ssim").unwrap();
        let requirements = converted.get(&pkg_name).expect("package should be present");
        assert_eq!(requirements.len(), 1);
        assert_eq!(requirements[0].requirement.name.as_ref(), "torch");
        assert!(!requirements[0].match_runtime);
    }

    #[test]
    fn empty_extra_build_dependencies_is_noop() {
        let converted = convert_extra_build_dependencies(&None, std::path::Path::new(".")).unwrap();
        assert!(converted.is_empty());
    }
}
