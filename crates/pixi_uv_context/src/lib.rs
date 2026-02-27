use std::sync::Arc;
use std::time::Duration;

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
    /// HTTP timeout for uv operations, read from UV_HTTP_TIMEOUT,
    /// UV_REQUEST_TIMEOUT, or HTTP_TIMEOUT environment variables.
    pub http_timeout: Option<Duration>,
}

/// Read the HTTP timeout from environment variables.
///
/// Checks `UV_HTTP_TIMEOUT`, `UV_REQUEST_TIMEOUT`, and `HTTP_TIMEOUT`
/// (in that order of precedence), matching the behavior of the `uv` CLI.
/// The value should be a number of seconds (e.g., `300` for 5 minutes).
fn read_http_timeout_from_env() -> Option<Duration> {
    let env_vars = ["UV_HTTP_TIMEOUT", "UV_REQUEST_TIMEOUT", "HTTP_TIMEOUT"];
    for var in env_vars {
        if let Ok(val) = std::env::var(var) {
            match val.parse::<u64>() {
                Ok(secs) => {
                    debug!("using {var}={secs}s for HTTP timeout");
                    return Some(Duration::from_secs(secs));
                }
                Err(_) => {
                    // Also try parsing as float for values like "30.5"
                    match val.parse::<f64>() {
                        Ok(secs) if secs >= 0.0 => {
                            debug!("using {var}={secs}s for HTTP timeout");
                            return Some(Duration::from_secs_f64(secs));
                        }
                        _ => {
                            tracing::warn!(
                                "ignoring invalid value for {var}: {val:?} (expected a number of seconds)"
                            );
                        }
                    }
                }
            }
        }
    }
    None
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
        let http_timeout = read_http_timeout_from_env();

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
            http_timeout,
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

        if let Some(timeout) = self.http_timeout {
            base_client_builder = base_client_builder.timeout(timeout);
        }

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
    use super::*;
    use std::sync::Mutex;

    // Mutex to prevent env var tests from interfering with each other
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// # Safety
    /// This function manipulates environment variables which is inherently
    /// unsafe in a multi-threaded context. The ENV_MUTEX must be held by the
    /// caller (enforced by requiring it in this module's test helpers).
    fn with_env_vars<F, R>(vars: &[(&str, Option<&str>)], f: F) -> R
    where
        F: FnOnce() -> R,
    {
        let _lock = ENV_MUTEX.lock().unwrap();
        // Save original values
        let originals: Vec<_> = vars
            .iter()
            .map(|(k, _)| (*k, std::env::var(k).ok()))
            .collect();

        // SAFETY: We hold ENV_MUTEX to ensure no concurrent env var access
        // within these tests.
        unsafe {
            for (k, v) in vars {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }

        let result = f();

        // SAFETY: Same as above - restoring original values under mutex.
        unsafe {
            for (k, v) in &originals {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }

        result
    }

    #[test]
    fn test_no_env_vars_returns_none() {
        with_env_vars(
            &[
                ("UV_HTTP_TIMEOUT", None),
                ("UV_REQUEST_TIMEOUT", None),
                ("HTTP_TIMEOUT", None),
            ],
            || {
                assert!(read_http_timeout_from_env().is_none());
            },
        );
    }

    #[test]
    fn test_uv_http_timeout_integer() {
        with_env_vars(
            &[
                ("UV_HTTP_TIMEOUT", Some("300")),
                ("UV_REQUEST_TIMEOUT", None),
                ("HTTP_TIMEOUT", None),
            ],
            || {
                assert_eq!(
                    read_http_timeout_from_env(),
                    Some(Duration::from_secs(300))
                );
            },
        );
    }

    #[test]
    fn test_uv_http_timeout_takes_precedence() {
        with_env_vars(
            &[
                ("UV_HTTP_TIMEOUT", Some("100")),
                ("UV_REQUEST_TIMEOUT", Some("200")),
                ("HTTP_TIMEOUT", Some("300")),
            ],
            || {
                assert_eq!(
                    read_http_timeout_from_env(),
                    Some(Duration::from_secs(100))
                );
            },
        );
    }

    #[test]
    fn test_uv_request_timeout_fallback() {
        with_env_vars(
            &[
                ("UV_HTTP_TIMEOUT", None),
                ("UV_REQUEST_TIMEOUT", Some("200")),
                ("HTTP_TIMEOUT", Some("300")),
            ],
            || {
                assert_eq!(
                    read_http_timeout_from_env(),
                    Some(Duration::from_secs(200))
                );
            },
        );
    }

    #[test]
    fn test_http_timeout_last_fallback() {
        with_env_vars(
            &[
                ("UV_HTTP_TIMEOUT", None),
                ("UV_REQUEST_TIMEOUT", None),
                ("HTTP_TIMEOUT", Some("300")),
            ],
            || {
                assert_eq!(
                    read_http_timeout_from_env(),
                    Some(Duration::from_secs(300))
                );
            },
        );
    }

    #[test]
    fn test_invalid_value_is_ignored() {
        with_env_vars(
            &[
                ("UV_HTTP_TIMEOUT", Some("not_a_number")),
                ("UV_REQUEST_TIMEOUT", Some("200")),
                ("HTTP_TIMEOUT", None),
            ],
            || {
                // Invalid UV_HTTP_TIMEOUT is skipped, falls back to UV_REQUEST_TIMEOUT
                assert_eq!(
                    read_http_timeout_from_env(),
                    Some(Duration::from_secs(200))
                );
            },
        );
    }

    #[test]
    fn test_float_seconds() {
        with_env_vars(
            &[
                ("UV_HTTP_TIMEOUT", Some("30.5")),
                ("UV_REQUEST_TIMEOUT", None),
                ("HTTP_TIMEOUT", None),
            ],
            || {
                let timeout = read_http_timeout_from_env().unwrap();
                assert_eq!(timeout, Duration::from_secs_f64(30.5));
            },
        );
    }
}
