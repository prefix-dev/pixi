use std::sync::Arc;
use std::time::Duration;

use fs_err::create_dir_all;
use miette::{Context, IntoDiagnostic};
use pixi_config::{self, CacheKind, Config};
use pixi_utils::reqwest::{LazyReqwestClient, should_use_system_certs_for_uv, uv_middlewares};
use pixi_uv_conversions::{ConversionError, to_uv_trusted_host};
use uv_cache::Cache;
use uv_client::{
    BaseClientBuilder, Connectivity, ExtraMiddleware, RegistryClient, RegistryClientBuilder,
};
use uv_configuration::{Concurrency, IndexStrategy, NoSources, TrustedHost};
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
    pub no_sources: NoSources,
    pub capabilities: IndexCapabilities,
    pub allow_insecure_host: Vec<TrustedHost>,
    pub shared_state: SharedState,
    pub extra_middleware: ExtraMiddleware,
    pub proxies: Vec<reqwest::Proxy>,
    pub tls_no_verify: bool,
    /// The shared reqwest client passed to uv via
    /// [`uv_client::BaseClientBuilder::custom_client`] when
    /// `allow_insecure_host` is empty. Lets uv reuse pixi's TLS configuration,
    /// proxies, mirrors, and connection pool instead of building its own.
    pub client: reqwest::Client,
    /// Fallback for the per-host TLS-bypass case: when `allow_insecure_host`
    /// is non-empty we let uv build its own dual (secure + dangerous) clients
    /// via [`uv_client::BaseClientBuilder::with_system_certs`], since reqwest
    /// 0.13 doesn't expose per-host TLS opt-in on a single `Client`.
    pub use_system_certs: bool,
    pub package_config_settings: PackageConfigSettings,
    pub extra_build_requires: ExtraBuildRequires,
    pub extra_build_variables: ExtraBuildVariables,
    pub preview: Preview,
    pub workspace_cache: WorkspaceCache,
    /// HTTP timeout for uv operations, read from UV_HTTP_TIMEOUT,
    /// UV_REQUEST_TIMEOUT, or HTTP_TIMEOUT environment variables.
    pub http_timeout: Option<Duration>,
    /// HTTP retry count for uv operations, read from UV_HTTP_RETRIES.
    pub http_retries: Option<u32>,
}

/// Read a `usize` from an environment variable, logging on success or invalid
/// values.
fn read_usize_env(var: &str) -> Option<usize> {
    let val = std::env::var(var).ok()?;
    match val.parse::<usize>() {
        Ok(n) if n > 0 => {
            tracing::debug!("using {var}={n}");
            Some(n)
        }
        _ => {
            tracing::warn!(
                "ignoring invalid value for {var}: {val:?} (expected a positive integer)"
            );
            None
        }
    }
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
                    tracing::debug!("using {var}={secs}s for HTTP timeout");
                    return Some(Duration::from_secs(secs));
                }
                Err(_) => {
                    // Also try parsing as float for values like "30.5"
                    match val.parse::<f64>() {
                        Ok(secs) if secs >= 0.0 => {
                            tracing::debug!("using {var}={secs}s for HTTP timeout");
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

/// Read `UV_HTTP_RETRIES` from the environment.
///
/// The value should be a non-negative integer (e.g., `5`). The default in uv
/// is 3.
fn read_http_retries_from_env() -> Option<u32> {
    let val = std::env::var("UV_HTTP_RETRIES").ok()?;
    match val.parse::<u32>() {
        Ok(n) => {
            tracing::debug!("using UV_HTTP_RETRIES={n}");
            Some(n)
        }
        Err(_) => {
            tracing::warn!(
                "ignoring invalid value for UV_HTTP_RETRIES: {val:?} (expected a non-negative integer)"
            );
            None
        }
    }
}

/// Build a [`Concurrency`] from pixi config and UV environment variables.
///
/// Precedence (highest wins):
/// 1. `UV_CONCURRENT_DOWNLOADS` / `UV_CONCURRENT_BUILDS` /
///    `UV_CONCURRENT_INSTALLS` environment variables
/// 2. Pixi `concurrency.downloads` config value
/// 3. uv defaults (50 downloads, system threads for builds/installs)
fn build_concurrency(config: &Config) -> Concurrency {
    let defaults = Concurrency::default();

    // Start with pixi config for downloads (it defaults to 50, same as uv)
    let downloads = config.max_concurrent_downloads();

    // Apply UV_ env var overrides
    let downloads = read_usize_env("UV_CONCURRENT_DOWNLOADS").unwrap_or(downloads);
    let builds = read_usize_env("UV_CONCURRENT_BUILDS").unwrap_or(defaults.builds);
    let installs = read_usize_env("UV_CONCURRENT_INSTALLS").unwrap_or(defaults.installs);

    Concurrency::new(downloads, builds, installs)
}

impl UvResolutionContext {
    pub fn from_config(config: &Config, client: LazyReqwestClient) -> miette::Result<Self> {
        let uv_cache = config.cache_dir_for(CacheKind::PypiWheels)?;
        if !uv_cache.exists() {
            create_dir_all(&uv_cache)
                .into_diagnostic()
                .context("failed to create uv cache directory")?;
        }

        let cache = Cache::from_path(uv_cache);

        let keyring_provider = match config.pypi_config.use_keyring() {
            pixi_config::KeyringProvider::Subprocess => {
                tracing::debug!("using uv keyring (subprocess) provider");
                uv_configuration::KeyringProviderType::Subprocess
            }
            pixi_config::KeyringProvider::Disabled => {
                tracing::debug!("uv keyring provider is disabled");
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
        let http_retries = read_http_retries_from_env();
        let concurrency = build_concurrency(config);

        Ok(Self {
            cache,
            in_flight: InFlight::default(),
            hash_strategy: HashStrategy::None,
            keyring_provider,
            concurrency,
            no_sources: NoSources::None,
            capabilities: IndexCapabilities::default(),
            allow_insecure_host,
            shared_state: SharedState::default(),
            extra_middleware: ExtraMiddleware(uv_middlewares(config, client.clone())),
            proxies: config.get_proxies().into_diagnostic()?,
            tls_no_verify: config.tls_no_verify(),
            client: client.into_client(),
            use_system_certs: should_use_system_certs_for_uv(config),
            package_config_settings: PackageConfigSettings::default(),
            extra_build_requires: ExtraBuildRequires::default(),
            extra_build_variables: ExtraBuildVariables::default(),
            preview: Preview::default(),
            workspace_cache: WorkspaceCache::default(),
            http_timeout,
            http_retries,
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

    /// Build the [`BaseClientBuilder`] used by every uv flow in pixi.
    ///
    /// All registry-client and resolver-satisfaction code paths construct
    /// their `BaseClientBuilder` here so TLS, proxy, middleware, and timeout
    /// configuration is set in one place.
    ///
    /// The HTTP client itself is selected based on `allow_insecure_hosts`:
    ///
    /// - **Empty (default):** uv reuses pixi's `reqwest::Client` via
    ///   `custom_client`, sharing pixi's TLS roots, proxies, mirror
    ///   middleware, and connection pool.
    /// - **Non-empty:** uv builds its own pair of clients via
    ///   `with_system_certs`, because it needs a separate
    ///   `tls_danger_accept_invalid_certs(true)` client for the flagged
    ///   hosts and reqwest 0.13 has no per-host TLS opt-in on a single
    ///   `Client`.
    pub fn base_client_builder<'a>(
        &self,
        allow_insecure_hosts: Vec<TrustedHost>,
        markers: Option<&'a MarkerEnvironment>,
        connectivity: Connectivity,
    ) -> BaseClientBuilder<'a> {
        let mut builder = BaseClientBuilder::default()
            .keyring(self.keyring_provider)
            .connectivity(connectivity)
            .extra_middleware(self.extra_middleware.clone());
        builder = if allow_insecure_hosts.is_empty() {
            builder
                .allow_insecure_host(allow_insecure_hosts)
                .custom_client(self.client.clone())
        } else {
            builder
                .allow_insecure_host(allow_insecure_hosts)
                .with_system_certs(self.use_system_certs)
        };
        if let Some(timeout) = self.http_timeout {
            builder = builder.read_timeout(timeout);
        }
        if let Some(retries) = self.http_retries {
            builder = builder.retries(retries);
        }
        if let Some(markers) = markers {
            builder = builder.markers(markers);
        }
        builder
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
        let base_client_builder =
            self.base_client_builder(allow_insecure_hosts, markers, connectivity);

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

    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    fn with_env_vars<F, R>(vars: &[(&str, Option<&str>)], f: F) -> R
    where
        F: FnOnce() -> R,
    {
        let _lock = ENV_MUTEX.lock().unwrap();
        let originals: Vec<_> = vars
            .iter()
            .map(|(k, _)| (*k, std::env::var(k).ok()))
            .collect();

        // SAFETY: We hold ENV_MUTEX to ensure no concurrent env var access.
        unsafe {
            for (k, v) in vars {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }

        let result = f();

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

    /// Clear all timeout-related env vars for a clean test.
    const TIMEOUT_VARS: [(&str, Option<&str>); 3] = [
        ("UV_HTTP_TIMEOUT", None),
        ("UV_REQUEST_TIMEOUT", None),
        ("HTTP_TIMEOUT", None),
    ];

    fn timeout_vars_with<'a>(
        overrides: &'a [(&'a str, &'a str)],
    ) -> Vec<(&'a str, Option<&'a str>)> {
        TIMEOUT_VARS
            .iter()
            .map(|&(k, _)| {
                let val = overrides.iter().find(|(ok, _)| *ok == k).map(|(_, v)| *v);
                (k, val)
            })
            .collect()
    }

    #[test]
    fn test_http_timeout_precedence_and_parsing() {
        // No env vars → None
        with_env_vars(&TIMEOUT_VARS, || {
            assert!(read_http_timeout_from_env().is_none());
        });

        // UV_HTTP_TIMEOUT takes precedence over the others
        with_env_vars(
            &[
                ("UV_HTTP_TIMEOUT", Some("100")),
                ("UV_REQUEST_TIMEOUT", Some("200")),
                ("HTTP_TIMEOUT", Some("300")),
            ],
            || {
                assert_eq!(read_http_timeout_from_env(), Some(Duration::from_secs(100)));
            },
        );

        // Falls through to UV_REQUEST_TIMEOUT, then HTTP_TIMEOUT
        with_env_vars(&timeout_vars_with(&[("UV_REQUEST_TIMEOUT", "200")]), || {
            assert_eq!(read_http_timeout_from_env(), Some(Duration::from_secs(200)));
        });
        with_env_vars(&timeout_vars_with(&[("HTTP_TIMEOUT", "300")]), || {
            assert_eq!(read_http_timeout_from_env(), Some(Duration::from_secs(300)));
        });

        // Invalid value is skipped, falls through to next
        with_env_vars(
            &[
                ("UV_HTTP_TIMEOUT", Some("nope")),
                ("UV_REQUEST_TIMEOUT", Some("200")),
                ("HTTP_TIMEOUT", None),
            ],
            || {
                assert_eq!(read_http_timeout_from_env(), Some(Duration::from_secs(200)));
            },
        );

        // Float seconds work
        with_env_vars(&timeout_vars_with(&[("UV_HTTP_TIMEOUT", "30.5")]), || {
            assert_eq!(
                read_http_timeout_from_env(),
                Some(Duration::from_secs_f64(30.5))
            );
        });
    }

    #[test]
    fn test_http_retries() {
        with_env_vars(&[("UV_HTTP_RETRIES", None)], || {
            assert!(read_http_retries_from_env().is_none());
        });
        with_env_vars(&[("UV_HTTP_RETRIES", Some("5"))], || {
            assert_eq!(read_http_retries_from_env(), Some(5));
        });
        with_env_vars(&[("UV_HTTP_RETRIES", Some("0"))], || {
            assert_eq!(read_http_retries_from_env(), Some(0));
        });
        with_env_vars(&[("UV_HTTP_RETRIES", Some("abc"))], || {
            assert!(read_http_retries_from_env().is_none());
        });
    }

    #[test]
    fn test_read_usize_env() {
        with_env_vars(&[("UV_CONCURRENT_DOWNLOADS", Some("10"))], || {
            assert_eq!(read_usize_env("UV_CONCURRENT_DOWNLOADS"), Some(10));
        });
        // Zero and invalid values are rejected
        for bad in ["0", "-1", "abc"] {
            with_env_vars(&[("UV_CONCURRENT_DOWNLOADS", Some(bad))], || {
                assert!(
                    read_usize_env("UV_CONCURRENT_DOWNLOADS").is_none(),
                    "expected None for {bad:?}"
                );
            });
        }
        // Unset → None
        with_env_vars(&[("UV_CONCURRENT_DOWNLOADS", None)], || {
            assert!(read_usize_env("UV_CONCURRENT_DOWNLOADS").is_none());
        });
    }
}
