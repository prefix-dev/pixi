use std::{
    collections::{BTreeSet as Set, HashMap},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str::FromStr,
    sync::LazyLock,
};

use clap::{ArgAction, Parser};
use itertools::Itertools;
use miette::{Context, IntoDiagnostic, miette};
use pixi_consts::consts;
use rattler_conda_types::{
    ChannelConfig, NamedChannelOrUrl, Platform, Version, VersionBumpType, VersionSpec,
    compression_level::CompressionLevel,
    package::CondaArchiveType,
    version_spec::{EqualityOperator, LogicalOperator, RangeOperator},
};
use rattler_networking::s3_middleware;
use rattler_repodata_gateway::{Gateway, GatewayBuilder, SourceConfig};
use reqwest::{NoProxy, Proxy};
use serde::{
    Deserialize, Serialize,
    de::{Error, IntoDeserializer},
};
use url::Url;

const EXPERIMENTAL: &str = "experimental";

/// Controls which root certificates to use for TLS connections.
///
/// - `Webpki`: Use bundled Mozilla root certificates (portable, works everywhere)
/// - `Native`: Use the system's certificate store (includes corporate CAs)
/// - `All`: Use both webpki and native certificates (union of both sources)
///
/// Note: This setting only has an effect when pixi is built with the `rustls-tls` feature.
/// When built with `native-tls`, system certificates are always used regardless of this setting.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TlsRootCerts {
    /// Use bundled Mozilla root certificates
    Webpki,
    /// Use the system's native certificate store
    Native,
    /// Use both webpki and native certificates
    #[default]
    All,
}

impl FromStr for TlsRootCerts {
    type Err = serde::de::value::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::deserialize(s.into_deserializer())
    }
}

impl std::fmt::Display for TlsRootCerts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TlsRootCerts::Webpki => write!(f, "webpki"),
            TlsRootCerts::Native => write!(f, "native"),
            TlsRootCerts::All => write!(f, "all"),
        }
    }
}

pub fn default_channel_config() -> ChannelConfig {
    ChannelConfig::default_with_root_dir(
        std::env::current_dir().expect("Could not retrieve the current directory"),
    )
}

/// Determines the default author based on the default git author. Both the name
/// and the email address of the author are returned.
pub fn get_default_author() -> Option<(String, String)> {
    let rv = Command::new("git")
        .arg("config")
        .arg("--get-regexp")
        .arg("^user.(name|email)$")
        .stdout(Stdio::piped())
        .output()
        .ok()?;

    let mut name = None;
    let mut email = None;

    for line in std::str::from_utf8(&rv.stdout).ok()?.lines() {
        match line.split_once(' ') {
            Some(("user.email", value)) => {
                email = Some(value.to_string());
            }
            Some(("user.name", value)) => {
                name = Some(value.to_string());
            }
            _ => {}
        }
    }

    Some((name?, email.unwrap_or_else(|| "".into())))
}

// detect proxy env vars like curl: https://curl.se/docs/manpage.html
static ENV_HTTP_PROXY: LazyLock<Option<String>> = LazyLock::new(|| {
    ["http_proxy", "all_proxy", "ALL_PROXY"]
        .iter()
        .find_map(|&k| std::env::var(k).ok().filter(|v| !v.is_empty()))
});
static ENV_HTTPS_PROXY: LazyLock<Option<String>> = LazyLock::new(|| {
    ["https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"]
        .iter()
        .find_map(|&k| std::env::var(k).ok().filter(|v| !v.is_empty()))
});
static ENV_NO_PROXY: LazyLock<Option<String>> = LazyLock::new(|| {
    ["no_proxy", "NO_PROXY"]
        .iter()
        .find_map(|&k| std::env::var(k).ok().filter(|v| !v.is_empty()))
});
static USE_PROXY_FROM_ENV: LazyLock<bool> =
    LazyLock::new(|| (*ENV_HTTPS_PROXY).is_some() || (*ENV_HTTP_PROXY).is_some());

/// Get pixi home directory, default to `$HOME/.pixi`
///
/// It may be overridden by the `PIXI_HOME` environment variable.
///
/// # Returns
///
/// The pixi home directory
pub fn pixi_home() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PIXI_HOME") {
        Some(PathBuf::from(path))
    } else {
        dirs::home_dir().map(|path| path.join(consts::PIXI_DIR))
    }
}

// TODO(tim): I think we should move this to another crate, dont know if global
// config is really correct
/// Returns the default cache directory.
/// Most important is the `PIXI_CACHE_DIR` environment variable.
/// - If that is not set, the `RATTLER_CACHE_DIR` environment variable is used.
/// - If that is not set, `XDG_CACHE_HOME/pixi` is used when the directory
///   exists.
/// - If that is not set, the default cache directory of
///   [`rattler::default_cache_dir`] is used.
pub fn get_cache_dir() -> miette::Result<PathBuf> {
    std::env::var("PIXI_CACHE_DIR")
        .ok()
        .map(PathBuf::from)
        .or_else(|| std::env::var("RATTLER_CACHE_DIR").map(PathBuf::from).ok())
        .or_else(|| {
            let pixi_cache_dir = dirs::cache_dir().map(|d| d.join(consts::PIXI_DIR));
            // Only use the xdg cache pixi directory when it exists
            pixi_cache_dir.and_then(|d| d.exists().then_some(d))
        })
        .or_else(|| rattler::default_cache_dir().ok())
        .ok_or_else(|| miette::miette!("could not determine default cache directory"))
}
#[derive(Parser, Debug, Default, Clone)]
pub struct ConfigCli {
    /// Path to the file containing the authentication token.
    #[arg(long, help_heading = consts::CLAP_CONFIG_OPTIONS)]
    auth_file: Option<PathBuf>,

    /// Max concurrent network requests, default is `50`
    #[arg(long, help_heading = consts::CLAP_CONFIG_OPTIONS)]
    concurrent_downloads: Option<usize>,

    /// Max concurrent solves, default is the number of CPUs
    #[arg(long, help_heading = consts::CLAP_CONFIG_OPTIONS)]
    concurrent_solves: Option<usize>,

    /// Set pinning strategy
    #[arg(long, help_heading = consts::CLAP_CONFIG_OPTIONS, value_enum)]
    pinning_strategy: Option<PinningStrategy>,

    /// Specifies whether to use the keyring to look up credentials for PyPI.
    #[arg(long, help_heading = consts::CLAP_CONFIG_OPTIONS)]
    pypi_keyring_provider: Option<KeyringProvider>,

    /// Run post-link scripts (insecure)
    #[arg(long, help_heading = consts::CLAP_CONFIG_OPTIONS)]
    run_post_link_scripts: bool,

    /// Do not verify the TLS certificate of the server.
    #[arg(long, action = ArgAction::SetTrue, help_heading = consts::CLAP_CONFIG_OPTIONS)]
    tls_no_verify: bool,

    /// Which TLS root certificates to use: 'webpki' (bundled Mozilla roots), 'native' (system store), or 'all' (both).
    #[arg(long, env = "PIXI_TLS_ROOT_CERTS", help_heading = consts::CLAP_CONFIG_OPTIONS)]
    tls_root_certs: Option<TlsRootCerts>,

    /// Use environment activation cache (experimental)
    #[arg(long, help_heading = consts::CLAP_CONFIG_OPTIONS)]
    use_environment_activation_cache: bool,
}

#[derive(Parser, Debug, Clone, Default)]
pub struct ConfigCliPrompt {
    /// Do not change the PS1 variable when starting a prompt.
    #[arg(long, help_heading = consts::CLAP_CONFIG_OPTIONS)]
    change_ps1: Option<bool>,
}

impl From<ConfigCliPrompt> for Config {
    fn from(cli: ConfigCliPrompt) -> Self {
        Self {
            shell: ShellConfig {
                change_ps1: cli.change_ps1,
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

impl ConfigCliPrompt {
    pub fn merge_config(self, config: Config) -> Config {
        let mut config = config;
        config.shell.change_ps1 = self.change_ps1.or(config.shell.change_ps1);
        config
    }
}

#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct RepodataConfig {
    #[serde(flatten)]
    pub default: RepodataChannelConfig,

    #[serde(flatten)]
    pub per_channel: HashMap<Url, RepodataChannelConfig>,
}

impl RepodataConfig {
    pub fn is_empty(&self) -> bool {
        self.default.is_empty() && self.per_channel.is_empty()
    }

    /// Merge the given RepodataConfig into the current one.
    /// `other` is mutable to allow for moving the values out of it.
    /// The given config will have higher priority
    pub fn merge(&self, mut other: Self) -> Self {
        let mut per_channel: HashMap<_, _> = self
            .per_channel
            .clone()
            .into_iter()
            .map(|(url, config)| {
                let other_config = other.per_channel.remove(&url).unwrap_or_default();
                (url, config.merge(other_config))
            })
            .collect();

        per_channel.extend(other.per_channel);

        Self {
            default: self.default.merge(other.default),
            per_channel,
        }
    }
}

#[derive(Parser, Debug, Default, Clone)]
pub struct ConfigCliActivation {
    /// Do not use the environment activation cache. (default: true except in
    /// experimental mode)
    #[arg(long, help_heading = consts::CLAP_CONFIG_OPTIONS)]
    force_activate: bool,

    /// Do not source the autocompletion scripts from the environment.
    #[arg(long, help_heading = consts::CLAP_CONFIG_OPTIONS)]
    no_completions: bool,
}

impl ConfigCliActivation {
    pub fn merge_config(self, config: Config) -> Config {
        let mut config = config;
        config.shell.force_activate = Some(self.force_activate);
        if self.no_completions {
            config.shell.source_completion_scripts = Some(false);
        }
        config
    }
}

#[derive(Clone, Default, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct RepodataChannelConfig {
    /// Disable JLAP compression for repodata.
    #[serde(alias = "disable_jlap")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_jlap: Option<bool>,
    /// Disable bzip2 compression for repodata.
    #[serde(alias = "disable_bzip2")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_bzip2: Option<bool>,
    /// Disable zstd compression for repodata.
    #[serde(alias = "disable_zstd")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_zstd: Option<bool>,
    /// Disable the use of sharded repodata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_sharded: Option<bool>,
}

impl RepodataChannelConfig {
    pub fn is_empty(&self) -> bool {
        self.disable_jlap.is_none()
            && self.disable_bzip2.is_none()
            && self.disable_zstd.is_none()
            && self.disable_sharded.is_none()
    }

    pub fn merge(&self, other: Self) -> Self {
        Self {
            disable_jlap: self.disable_jlap.or(other.disable_jlap),
            disable_zstd: self.disable_zstd.or(other.disable_zstd),
            disable_bzip2: self.disable_bzip2.or(other.disable_bzip2),
            disable_sharded: self.disable_sharded.or(other.disable_sharded),
        }
    }
}

impl From<RepodataChannelConfig> for SourceConfig {
    fn from(value: RepodataChannelConfig) -> Self {
        SourceConfig {
            jlap_enabled: !value.disable_jlap.unwrap_or(true),
            zstd_enabled: !value.disable_zstd.unwrap_or(false),
            bz2_enabled: !value.disable_bzip2.unwrap_or(false),
            sharded_enabled: !value.disable_sharded.unwrap_or(false),
            cache_action: Default::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum KeyringProvider {
    Disabled,
    Subprocess,
}

#[derive(Clone, Debug, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct PyPIConfig {
    /// The default index URL for PyPI packages.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_url: Option<Url>,
    /// A list of extra index URLs for PyPI packages
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extra_index_urls: Vec<Url>,
    /// Whether to use the `keyring` executable to look up credentials.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyring_provider: Option<KeyringProvider>,
    /// Allow insecure connections to a host
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allow_insecure_host: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct S3Options {
    /// S3 endpoint URL
    pub endpoint_url: Url,

    /// The name of the S3 region
    pub region: String,

    /// Force path style URLs instead of subdomain style
    pub force_path_style: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum DetachedEnvironments {
    Boolean(bool),
    Path(PathBuf),
}

impl DetachedEnvironments {
    pub fn is_false(&self) -> bool {
        matches!(self, DetachedEnvironments::Boolean(false))
    }

    // Get the path to the detached-environments directory. None means the default
    // directory.
    pub fn path(&self) -> miette::Result<Option<PathBuf>> {
        let resolved_self = self.resolve_path()?;
        match resolved_self {
            DetachedEnvironments::Path(p) => Ok(Some(p.clone())),
            DetachedEnvironments::Boolean(b) if b => {
                let path = get_cache_dir()?.join(consts::ENVIRONMENTS_DIR);
                Ok(Some(path))
            }
            _ => Ok(None),
        }
    }

    /// If `self` is the `DetachedEnvironments::Path` variant, expands `~`
    /// to the absolute path to the home directory, otherwise clone the boolean
    /// variant.
    pub fn resolve_path(&self) -> miette::Result<Self> {
        match self {
            DetachedEnvironments::Boolean(_) => Ok(self.clone()),
            DetachedEnvironments::Path(p) => {
                let mut path = p.clone();
                // If the path starts with ~, expand it to the home directory
                if path.to_string_lossy().starts_with("~") {
                    let home_dir = dirs::home_dir().ok_or_else(|| {
                        miette!(
                            "Could not resolve home directory for '~' in path {}",
                            path.display()
                        )
                    })?;
                    // Safe unwrap as we checked if it starts with ~
                    path = home_dir.join(path.strip_prefix("~").unwrap());
                }
                Ok(DetachedEnvironments::Path(path))
            }
        }
    }

    pub fn validate(&self) -> miette::Result<()> {
        // Resolve the path variant (if present) prior to validating it.
        let resolved_self = self.resolve_path()?;

        match resolved_self {
            DetachedEnvironments::Boolean(_) => {}
            DetachedEnvironments::Path(path) => {
                if !path.is_absolute() {
                    return Err(miette!(
                        "The `detached-environments` path must be an absolute path: {}",
                        path.display()
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Default for DetachedEnvironments {
    fn default() -> Self {
        DetachedEnvironments::Boolean(false)
    }
}

#[derive(Default, Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ExperimentalConfig {
    /// The option to opt into the environment activation cache feature.
    /// This is an experimental feature and may be removed in the future or made
    /// default.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_environment_activation_cache: Option<bool>,
}

impl ExperimentalConfig {
    pub fn merge(self, other: Self) -> Self {
        Self {
            use_environment_activation_cache: other
                .use_environment_activation_cache
                .or(self.use_environment_activation_cache),
        }
    }
    pub fn use_environment_activation_cache(&self) -> bool {
        self.use_environment_activation_cache.unwrap_or(false)
    }

    pub fn is_default(&self) -> bool {
        self.use_environment_activation_cache.is_none()
    }
}

// Making the default values part of pixi_config to allow for printing the
// default settings in the future.
/// The default maximum number of concurrent solves that can be run at once.
/// Defaulting to the number of CPUs available.
fn default_max_concurrent_solves() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// The default maximum number of concurrent downloads that can be run at once.
/// 50 is a reasonable default for the number of concurrent downloads.
/// More verification is needed to determine the optimal number.
fn default_max_concurrent_downloads() -> usize {
    50
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ConcurrencyConfig {
    /// The maximum number of concurrent solves that can be run at once.
    // Needing to set this default next to the default of the full struct to avoid serde defaulting
    // to 0 of partial struct was omitted.
    #[serde(default = "default_max_concurrent_solves")]
    pub solves: usize,

    /// The maximum number of concurrent HTTP requests to make.
    // Needing to set this default next to the default of the full struct to avoid serde defaulting
    // to 0 of partial struct was omitted.
    #[serde(default = "default_max_concurrent_downloads")]
    pub downloads: usize,
}

impl Default for ConcurrencyConfig {
    fn default() -> Self {
        Self {
            solves: default_max_concurrent_solves(),
            downloads: default_max_concurrent_downloads(),
        }
    }
}

impl ConcurrencyConfig {
    /// Merge the given ConcurrencyConfig into the current one.
    pub fn merge(self, other: Self) -> Self {
        // Merging means using the other value if they are none default.
        Self {
            solves: if other.solves != ConcurrencyConfig::default().solves {
                other.solves
            } else {
                self.solves
            },
            downloads: if other.downloads != ConcurrencyConfig::default().downloads {
                other.downloads
            } else {
                self.downloads
            },
        }
    }

    pub fn is_default(&self) -> bool {
        ConcurrencyConfig::default() == *self
    }
}

impl PyPIConfig {
    /// Merge the given PyPIConfig into the current one.
    pub fn merge(self, other: Self) -> Self {
        let extra_index_urls = self
            .extra_index_urls
            .into_iter()
            .chain(other.extra_index_urls)
            .collect();

        Self {
            index_url: other.index_url.or(self.index_url),
            extra_index_urls,
            keyring_provider: other.keyring_provider.or(self.keyring_provider),
            allow_insecure_host: self
                .allow_insecure_host
                .into_iter()
                .chain(other.allow_insecure_host)
                .collect(),
        }
    }

    pub fn with_keyring(mut self, keyring_provider: KeyringProvider) -> Self {
        self.keyring_provider = Some(keyring_provider);
        self
    }

    /// Whether to use the `keyring` executable to look up credentials.
    /// Defaults to false.
    pub fn use_keyring(&self) -> KeyringProvider {
        self.keyring_provider
            .clone()
            .unwrap_or(KeyringProvider::Disabled)
    }

    fn is_default(&self) -> bool {
        self.index_url.is_none()
            && self.extra_index_urls.is_empty()
            && self.keyring_provider.is_none()
    }
}

/// The strategy for that will be used for pinning a version of a package.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, Copy, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum PinningStrategy {
    /// Default semver strategy e.g. `1.2.3` becomes `>=1.2.3, <2` but `0.1.0`
    /// becomes `>=0.1.0, <0.2`
    #[default]
    Semver,
    /// Pin the latest minor e.g. `1.2.3` becomes `>=1.2.3, <1.3`
    Minor,
    /// Pin the latest major e.g. `1.2.3` becomes `>=1.2.3, <2`
    Major,
    /// Pin to the latest version or higher. e.g. `1.2.3` becomes `>=1.2.3`
    LatestUp,
    /// Pin the version chosen by the solver. e.g. `1.2.3` becomes `==1.2.3`
    // Adding "Version" to the name for future extendability.
    ExactVersion,
    /// No pinning, keep the requirement empty. e.g. `1.2.3` becomes `*`
    // Calling it no-pin to make it simple to type, as other option was pin-unconstrained.
    NoPin,
}
impl FromStr for PinningStrategy {
    type Err = serde::de::value::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::deserialize(s.into_deserializer())
    }
}

impl PinningStrategy {
    /// Given a set of versions, determines the best version constraint to use
    /// that captures all of them based on the strategy.
    pub fn determine_version_constraint<'a>(
        self,
        versions: impl IntoIterator<Item = &'a Version> + Clone,
    ) -> Option<VersionSpec> {
        let (min_version, max_version) = versions.clone().into_iter().minmax().into_option()?;
        let lower_bound = min_version.clone();
        let num_segments = max_version.segment_count();

        let constraint = match self {
            Self::ExactVersion => VersionSpec::Group(
                LogicalOperator::Or,
                versions
                    .into_iter()
                    .dedup()
                    .map(|v| VersionSpec::Exact(EqualityOperator::Equals, v.clone()))
                    .collect(),
            ),
            Self::Major => {
                let upper_bound = max_version
                    .clone()
                    .pop_segments(num_segments.saturating_sub(1))
                    .unwrap_or(max_version.to_owned())
                    .bump(VersionBumpType::Major)
                    .ok()?;
                VersionSpec::Group(
                    LogicalOperator::And,
                    vec![
                        VersionSpec::Range(RangeOperator::GreaterEquals, lower_bound),
                        VersionSpec::Range(RangeOperator::Less, upper_bound),
                    ],
                )
            }
            Self::Minor => {
                let upper_bound = max_version
                    .clone()
                    .pop_segments(num_segments.saturating_sub(2))
                    .unwrap_or(max_version.to_owned())
                    .bump(VersionBumpType::Minor)
                    .ok()?;
                VersionSpec::Group(
                    LogicalOperator::And,
                    vec![
                        VersionSpec::Range(RangeOperator::GreaterEquals, lower_bound),
                        VersionSpec::Range(RangeOperator::Less, upper_bound),
                    ],
                )
            }
            Self::LatestUp => VersionSpec::Range(RangeOperator::GreaterEquals, lower_bound),
            Self::NoPin => VersionSpec::Any,
            Self::Semver => {
                // Pin the first left most non-zero segment for the upperbound
                let mut left_most_non_zero_offset = None;
                for (index, segment) in max_version.segments().enumerate() {
                    if !segment.is_zero() {
                        left_most_non_zero_offset = Some(index);
                        break;
                    }
                }
                let upper_bound = max_version
                    .with_segments(
                        0..=left_most_non_zero_offset.unwrap_or(max_version.segment_count()),
                    )
                    .unwrap_or(max_version.clone())
                    .bump(VersionBumpType::Last)
                    .ok()?;
                VersionSpec::Group(
                    LogicalOperator::And,
                    vec![
                        VersionSpec::Range(RangeOperator::GreaterEquals, lower_bound),
                        VersionSpec::Range(RangeOperator::Less, upper_bound),
                    ],
                )
            }
        };
        Some(constraint)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RunPostLinkScripts {
    /// Run the post link scripts, we call this insecure as it may run arbitrary
    /// code.
    Insecure,
    /// Do not run the post link scripts
    #[default]
    False,
}
impl FromStr for RunPostLinkScripts {
    type Err = serde::de::value::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::deserialize(s.into_deserializer())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct Config {
    #[serde(default)]
    #[serde(alias = "default_channels")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub default_channels: Vec<NamedChannelOrUrl>,

    /// Path to the file containing the authentication token.
    #[serde(default)]
    #[serde(alias = "authentication_override_file")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication_override_file: Option<PathBuf>,

    /// If set to true, pixi will not verify the TLS certificate of the server.
    #[serde(default)]
    #[serde(alias = "tls_no_verify")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_no_verify: Option<bool>,

    /// Which TLS root certificates to use for HTTPS connections.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_root_certs: Option<TlsRootCerts>,

    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub mirrors: HashMap<Url, Vec<Url>>,

    /// Dependency Pinning strategy used for dependency modification through
    /// automated logic like `pixi add`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinning_strategy: Option<PinningStrategy>,

    #[serde(skip)]
    #[serde(alias = "loaded_from")] // BREAK: remove to stop supporting snake_case alias
    pub loaded_from: Vec<PathBuf>,

    #[serde(skip, default = "default_channel_config")]
    pub channel_config: ChannelConfig,

    /// Configuration for repodata fetching.
    #[serde(alias = "repodata_config")] // BREAK: remove to stop supporting snake_case alias
    #[serde(default, skip_serializing_if = "RepodataConfig::is_empty")]
    pub repodata_config: RepodataConfig,

    /// Configuration for PyPI packages.
    #[serde(default)]
    #[serde(skip_serializing_if = "PyPIConfig::is_default")]
    pub pypi_config: PyPIConfig,

    /// Configuration for S3.
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub s3_options: HashMap<String, S3Options>,

    /// The option to specify the directory where detached environments are
    /// stored. When using 'true', it defaults to the cache directory.
    /// When using a path, it uses the specified path.
    /// When using 'false', it disables detached environments, meaning it moves
    /// it back to the .pixi folder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detached_environments: Option<DetachedEnvironments>,

    /// Shell-specific configuration
    #[serde(default)]
    #[serde(skip_serializing_if = "ShellConfig::is_default")]
    pub shell: ShellConfig,

    /// Experimental features that can be enabled.
    #[serde(default)]
    #[serde(skip_serializing_if = "ExperimentalConfig::is_default")]
    pub experimental: ExperimentalConfig,

    /// Concurrency configuration for pixi
    #[serde(default)]
    #[serde(skip_serializing_if = "ConcurrencyConfig::is_default")]
    pub concurrency: ConcurrencyConfig,

    /// Run the post link scripts
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_post_link_scripts: Option<RunPostLinkScripts>,

    /// Https/Http proxy configuration for pixi
    #[serde(default)]
    #[serde(skip_serializing_if = "ProxyConfig::is_default")]
    pub proxy_config: ProxyConfig,

    /// Build configuration for pixi and rattler-build
    #[serde(default)]
    #[serde(skip_serializing_if = "BuildConfig::is_default")]
    pub build: BuildConfig,

    /// The platform to use when installing tools.
    ///
    /// When running on certain platforms, you might want to install build
    /// backends and other tools for a different platform than the current one.
    /// Using this field, you can specify the platform that is used to install
    /// these types of tools.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_platform: Option<Platform>,

    //////////////////////
    // Deprecated fields //
    //////////////////////
    #[serde(default)]
    #[serde(alias = "change_ps1")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_ps1: Option<bool>,

    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force_activate: Option<bool>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_channels: Vec::new(),
            authentication_override_file: None,
            tls_no_verify: None,
            tls_root_certs: None,
            mirrors: HashMap::new(),
            loaded_from: Vec::new(),
            channel_config: default_channel_config(),
            repodata_config: RepodataConfig::default(),
            pypi_config: PyPIConfig::default(),
            s3_options: HashMap::new(),
            detached_environments: None,
            pinning_strategy: None,
            shell: ShellConfig::default(),
            experimental: ExperimentalConfig::default(),
            concurrency: ConcurrencyConfig::default(),
            run_post_link_scripts: None,
            proxy_config: ProxyConfig::default(),
            build: BuildConfig::default(),
            tool_platform: None,

            // Deprecated fields
            change_ps1: None,
            force_activate: None,
        }
    }
}

impl From<ConfigCli> for Config {
    fn from(cli: ConfigCli) -> Self {
        Self {
            tls_no_verify: if cli.tls_no_verify { Some(true) } else { None },
            tls_root_certs: cli.tls_root_certs,
            authentication_override_file: cli.auth_file,
            pypi_config: cli
                .pypi_keyring_provider
                .map(|val| PyPIConfig::default().with_keyring(val))
                .unwrap_or_default(),
            detached_environments: None,
            concurrency: ConcurrencyConfig {
                solves: cli
                    .concurrent_solves
                    .unwrap_or(ConcurrencyConfig::default().solves),
                downloads: cli
                    .concurrent_downloads
                    .unwrap_or(ConcurrencyConfig::default().downloads),
            },
            tool_platform: None,
            run_post_link_scripts: if cli.run_post_link_scripts {
                Some(RunPostLinkScripts::Insecure)
            } else {
                None
            },
            experimental: ExperimentalConfig {
                use_environment_activation_cache: if cli.use_environment_activation_cache {
                    Some(true)
                } else {
                    None
                },
            },
            pinning_strategy: cli.pinning_strategy,
            ..Default::default()
        }
    }
}

impl From<Config> for rattler_repodata_gateway::ChannelConfig {
    fn from(config: Config) -> Self {
        rattler_repodata_gateway::ChannelConfig::from(&config)
    }
}

impl From<&Config> for rattler_repodata_gateway::ChannelConfig {
    fn from(config: &Config) -> Self {
        let repodata_config = &config.repodata_config;
        let default = repodata_config.default.clone().into();

        let per_channel = repodata_config
            .per_channel
            .iter()
            .map(|(url, config)| {
                (
                    url.clone(),
                    config.merge(repodata_config.default.clone()).into(),
                )
            })
            .collect();

        rattler_repodata_gateway::ChannelConfig {
            default,
            per_channel,
        }
    }
}

#[derive(Clone, Default, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ShellConfig {
    /// The option to disable the environment activation cache
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force_activate: Option<bool>,

    /// Whether to source completion scripts from the environment or not.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    source_completion_scripts: Option<bool>,

    /// If set to true, pixi will set the PS1 environment variable to a custom
    /// value.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_ps1: Option<bool>,
}

impl ShellConfig {
    pub fn merge(self, other: Self) -> Self {
        Self {
            force_activate: other.force_activate.or(self.force_activate),
            source_completion_scripts: other
                .source_completion_scripts
                .or(self.source_completion_scripts),
            change_ps1: other.change_ps1.or(self.change_ps1),
        }
    }

    pub fn is_default(&self) -> bool {
        self.force_activate.is_none()
            && self.source_completion_scripts.is_none()
            && self.change_ps1.is_none()
    }

    pub fn source_completion_scripts(&self) -> bool {
        self.source_completion_scripts.unwrap_or(true)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ProxyConfig {
    /// https proxy.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub https: Option<Url>,
    /// http proxy.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http: Option<Url>,
    /// A list of no proxy pattern
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub non_proxy_hosts: Vec<String>,
}

impl ProxyConfig {
    pub fn is_default(&self) -> bool {
        self.https.is_none() && self.https.is_none() && self.non_proxy_hosts.is_empty()
    }
    pub fn merge(&self, other: Self) -> Self {
        Self {
            https: other.https.as_ref().or(self.https.as_ref()).cloned(),
            http: other.http.as_ref().or(self.http.as_ref()).cloned(),
            non_proxy_hosts: if other.is_default() {
                self.non_proxy_hosts.clone()
            } else {
                other.non_proxy_hosts.clone()
            },
        }
    }
}

/// Container for the package format and compression level
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PackageFormatAndCompression {
    /// The archive type that is selected
    pub archive_type: CondaArchiveType,
    /// The compression level that is selected
    pub compression_level: CompressionLevel,
}

// deserializer for the package format and compression level
impl<'de> Deserialize<'de> for PackageFormatAndCompression {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let s = s.as_str();
        PackageFormatAndCompression::from_str(s).map_err(D::Error::custom)
    }
}

impl FromStr for PackageFormatAndCompression {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut split = s.split(':');
        let package_format = split.next().ok_or("invalid")?;

        let compression = split.next().unwrap_or("default");

        // remove all non-alphanumeric characters
        let package_format = package_format
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>();

        let archive_type = match package_format.to_lowercase().as_str() {
            "tarbz2" => CondaArchiveType::TarBz2,
            "conda" => CondaArchiveType::Conda,
            _ => return Err(format!("Unknown package format: {package_format}")),
        };

        let compression_level = match compression {
            "max" | "highest" => CompressionLevel::Highest,
            "default" | "normal" => CompressionLevel::Default,
            "fast" | "lowest" | "min" => CompressionLevel::Lowest,
            number if number.parse::<i32>().is_ok() => {
                let number = number.parse::<i32>().unwrap_or_default();
                match archive_type {
                    CondaArchiveType::TarBz2 => {
                        if !(1..=9).contains(&number) {
                            return Err("Compression level for .tar.bz2 must be between 1 and 9"
                                .to_string());
                        }
                    }
                    CondaArchiveType::Conda => {
                        if !(-7..=22).contains(&number) {
                            return Err(
                                "Compression level for conda packages (zstd) must be between -7 and 22".to_string()
                            );
                        }
                    }
                }
                CompressionLevel::Numeric(number)
            }
            _ => return Err(format!("Unknown compression level: {compression}")),
        };

        Ok(PackageFormatAndCompression {
            archive_type,
            compression_level,
        })
    }
}

impl Serialize for PackageFormatAndCompression {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let package_format = match self.archive_type {
            CondaArchiveType::TarBz2 => "tarbz2",
            CondaArchiveType::Conda => "conda",
        };
        let compression_level = match self.compression_level {
            CompressionLevel::Default => "default",
            CompressionLevel::Highest => "max",
            CompressionLevel::Lowest => "min",
            CompressionLevel::Numeric(level) => &level.to_string(),
        };

        serializer.serialize_str(format!("{package_format}:{compression_level}").as_str())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct BuildConfig {
    /// package format and compression level
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_format: Option<PackageFormatAndCompression>,
}

impl BuildConfig {
    pub fn is_default(&self) -> bool {
        self.package_format.is_none()
    }
    pub fn merge(&self, other: Self) -> Self {
        Self {
            package_format: other
                .package_format
                .as_ref()
                .or(self.package_format.as_ref())
                .cloned(),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ConfigError {
    #[error("no file was found at {0}")]
    FileNotFound(PathBuf),
    #[error("failed to read config from '{0}'")]
    ReadError(std::io::Error),
    #[error("failed to parse config of {1}: {0}")]
    ParseError(miette::Report, PathBuf),
    #[error("validation error of {1}: {0}")]
    ValidationError(miette::Report, PathBuf),
}

impl Config {
    /// Constructs a new config that is optimized to be used in tests.
    ///
    /// This instance is optimized to provide the fastest experience for tests.
    pub fn for_tests() -> Self {
        let mut config = Config::default();
        // Use prefix.dev as the default channel alias
        config.channel_config.channel_alias = Url::parse("https://prefix.dev").unwrap();

        // Use conda-forge as the default channel
        config.default_channels = vec![NamedChannelOrUrl::Name("conda-forge".into())];

        // Enable sharded repodata by default.
        config.repodata_config.default.disable_sharded = Some(false);

        // HACK: Use win-64 as the default tool platform if currently running on
        // win-arm64. This is a workaround for the fact that we don't have a
        // good win-arm64 toolchain yet.
        if Platform::current() == Platform::WinArm64 {
            config.tool_platform = Some(Platform::Win64);
        }

        config
    }

    /// Parse the given toml string and return a Config instance.
    ///
    /// # Returns
    ///
    /// The parsed config, and the unused keys
    ///
    /// # Errors
    ///
    /// Parsing errors
    #[inline]
    pub fn from_toml(
        toml: &str,
        source_path: Option<&Path>,
    ) -> miette::Result<(Config, Set<String>)> {
        let de = toml_edit::de::Deserializer::from_str(toml).into_diagnostic()?;

        // Deserialize the config and collect unused keys
        let mut unused_keys = Set::new();
        let mut config: Config = serde_ignored::deserialize(de, |path| {
            unused_keys.insert(path.to_string());
        })
        .into_diagnostic()?;

        fn create_deprecation_warning(old: &str, new: &str, source_path: Option<&Path>) {
            let msg = format!(
                "Please replace '{}' with '{}', the field is deprecated and will be removed in a future release.",
                console::style(old).red(),
                console::style(new).green()
            );
            match source_path {
                Some(path) => {
                    tracing::warn!("In '{}': {}", console::style(path.display()).bold(), msg,)
                }
                None => tracing::warn!("{}", msg),
            }
        }

        if config.change_ps1.is_some() {
            create_deprecation_warning("change-ps1", "shell.change-ps1", source_path);
            config.shell.change_ps1 = config.change_ps1;
        }

        if config.force_activate.is_some() {
            create_deprecation_warning("force-activate", "shell.force-activate", source_path);
            config.shell.force_activate = config.force_activate;
        }

        Ok((config, unused_keys))
    }

    /// Load the config from the given path.
    ///
    /// # Returns
    ///
    /// The loaded config
    ///
    /// # Errors
    ///
    /// I/O errors or parsing errors
    pub fn from_path(path: &Path) -> Result<Config, ConfigError> {
        tracing::debug!("Loading config from {}", path.display());
        let s = match fs_err::read_to_string(path) {
            Ok(content) => content,
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    || e.kind() == std::io::ErrorKind::NotADirectory =>
            {
                return Err(ConfigError::FileNotFound(path.to_path_buf()));
            }
            Err(e) => return Err(ConfigError::ReadError(e)),
        };

        let (mut config, unused_keys) = Config::from_toml(&s, Some(path))
            .map_err(|e| ConfigError::ParseError(e, path.to_path_buf()))?;

        if !unused_keys.is_empty() {
            tracing::warn!(
                "Ignoring '{}' in at {}",
                console::style(
                    unused_keys
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
                .yellow(),
                path.display()
            );
        }

        config.loaded_from.push(path.to_path_buf());
        tracing::debug!("Loaded config from: {}", path.display());

        config
            .validate()
            .map_err(|e| ConfigError::ValidationError(e, path.to_path_buf()))?;

        // check proxy config
        if config.proxy_config.https.is_none() && config.proxy_config.http.is_none() {
            if !config.proxy_config.non_proxy_hosts.is_empty() {
                tracing::warn!(
                    "proxy-config.non-proxy-hosts is not empty but will be ignored, as no https or http config is set."
                )
            }
        } else if *USE_PROXY_FROM_ENV {
            let config_no_proxy = Some(config.proxy_config.non_proxy_hosts.iter().join(","))
                .filter(|v| !v.is_empty());
            if (*ENV_HTTPS_PROXY).as_deref() != config.proxy_config.https.as_ref().map(Url::as_str)
                || (*ENV_HTTP_PROXY).as_deref()
                    != config.proxy_config.http.as_ref().map(Url::as_str)
                || *ENV_NO_PROXY != config_no_proxy
            {
                tracing::info!("proxy configs are overridden by proxy environment vars.")
            }
        }

        Ok(config)
    }

    /// Try to load the system config file from the system path.
    ///
    /// # Returns
    ///
    /// The loaded system config
    ///
    /// # Errors
    ///
    /// I/O errors or parsing errors
    pub fn try_load_system() -> Result<Config, ConfigError> {
        Self::from_path(&config_path_system())
    }

    /// Load the system config file from the system path.
    ///
    /// # Returns
    ///
    /// The loaded system config
    pub fn load_system() -> Config {
        Self::try_load_system().unwrap_or_else(|e| {
            match e {
                ConfigError::FileNotFound(_) => (), // it's fine that no file is there
                e => tracing::error!("{e}"),
            }

            Self::default()
        })
    }

    /// Validate the config file.
    pub fn validate(&self) -> miette::Result<()> {
        // Validate the detached environments directory is set correctly
        if let Some(detached_environments) = self.detached_environments.as_ref() {
            detached_environments.validate()?
        }

        Ok(())
    }

    /// Load the global config file from various global paths.
    ///
    /// # Returns
    ///
    /// The loaded global config
    pub fn load_global() -> Config {
        let mut config = Self::load_system();

        for p in config_path_global() {
            match Self::from_path(&p) {
                Ok(c) => config = config.merge_config(c),
                Err(ConfigError::FileNotFound(_)) => (),
                Err(e) => tracing::error!(
                    "Failed to load global config '{}' with error: {}",
                    p.display(),
                    e
                ),
            }
        }

        // Load the default CLI config and layer it on top of the global config
        // This will add any environment variables defined in the `clap` attributes to
        // the config
        let mut default_cli = ConfigCli::default();
        default_cli.update_from(std::env::args().take(0));
        config.merge_config(default_cli.into())
    }

    /// Load the global config and layer the given cli config on top of it.
    pub fn with_cli_config(cli: &ConfigCli) -> Config {
        let config = Config::load_global();
        config.merge_config(cli.clone().into())
    }

    /// Load the config from the given path (project root).
    ///
    /// # Returns
    ///
    /// The loaded config (merged with the global config)
    pub fn load(project_root: &Path) -> Config {
        let mut config = Self::load_global();
        let local_config_path = project_root
            .join(consts::PIXI_DIR)
            .join(consts::CONFIG_FILE);

        match Self::from_path(&local_config_path) {
            Ok(c) => config = config.merge_config(c),
            Err(e) => tracing::debug!(
                "Failed to load local config: {} (error: {})",
                local_config_path.display(),
                e
            ),
        }

        config
    }

    // Get all possible keys of the configuration
    pub fn get_keys(&self) -> &[&str] {
        &[
            "authentication-override-file",
            "concurrency",
            "concurrency.downloads",
            "concurrency.solves",
            "default-channels",
            "detached-environments",
            "experimental",
            "experimental.use-environment-activation-cache",
            "mirrors",
            "pinning-strategy",
            "proxy-config",
            "proxy-config.http",
            "proxy-config.https",
            "proxy-config.non-proxy-hosts",
            "pypi-config",
            "pypi-config.allow-insecure-host",
            "pypi-config.extra-index-urls",
            "pypi-config.index-url",
            "pypi-config.keyring-provider",
            "repodata-config",
            "repodata-config.disable-bzip2",
            "repodata-config.disable-jlap",
            "repodata-config.disable-sharded",
            "repodata-config.disable-zstd",
            "run-post-link-scripts",
            "s3-options",
            "s3-options.<bucket>",
            "s3-options.<bucket>.endpoint-url",
            "s3-options.<bucket>.force-path-style",
            "s3-options.<bucket>.region",
            "shell",
            "shell.change-ps1",
            "shell.force-activate",
            "shell.source-completion-scripts",
            "tls-no-verify",
            "tls-root-certs",
            "tool-platform",
        ]
    }

    /// Merge the `other` config into `self`.
    /// The `other` config will have higher priority
    #[must_use]
    pub fn merge_config(mut self, mut other: Config) -> Self {
        self.mirrors.extend(other.mirrors);
        other.loaded_from.extend(self.loaded_from);

        Self {
            default_channels: if other.default_channels.is_empty() {
                self.default_channels
            } else {
                other.default_channels
            },
            tls_no_verify: other.tls_no_verify.or(self.tls_no_verify),
            tls_root_certs: other.tls_root_certs.or(self.tls_root_certs),
            authentication_override_file: other
                .authentication_override_file
                .or(self.authentication_override_file),
            // Extended self.mirrors with other.mirrors
            mirrors: self.mirrors,
            loaded_from: other.loaded_from,
            channel_config: if other.channel_config == default_channel_config() {
                self.channel_config
            } else {
                other.channel_config
            },
            repodata_config: self.repodata_config.merge(other.repodata_config),
            pypi_config: self.pypi_config.merge(other.pypi_config),
            s3_options: {
                let mut merged = HashMap::new();
                merged.extend(self.s3_options);
                merged.extend(other.s3_options);
                merged
            },
            detached_environments: other.detached_environments.or(self.detached_environments),
            pinning_strategy: other.pinning_strategy.or(self.pinning_strategy),
            shell: self.shell.merge(other.shell),
            experimental: self.experimental.merge(other.experimental),
            // Make other take precedence over self to allow for setting the value through the CLI
            concurrency: self.concurrency.merge(other.concurrency),
            run_post_link_scripts: other.run_post_link_scripts.or(self.run_post_link_scripts),

            proxy_config: self.proxy_config.merge(other.proxy_config),
            build: self.build.merge(other.build),
            tool_platform: self.tool_platform.or(other.tool_platform),

            // Deprecated fields that we can ignore as we handle them inside `shell.` field
            change_ps1: None,
            force_activate: None,
        }
    }

    /// Retrieve the value for the default_channels field (defaults to the
    /// ["conda-forge"]).
    pub fn default_channels(&self) -> Vec<NamedChannelOrUrl> {
        if self.default_channels.is_empty() {
            consts::DEFAULT_CHANNELS.clone()
        } else {
            self.default_channels.clone()
        }
    }

    /// Retrieve the value for the tls_no_verify field (defaults to false).
    pub fn tls_no_verify(&self) -> bool {
        self.tls_no_verify.unwrap_or(false)
    }

    /// Retrieve the value for the tls_root_certs field (defaults to Webpki).
    pub fn tls_root_certs(&self) -> TlsRootCerts {
        self.tls_root_certs.unwrap_or_default()
    }

    /// Retrieve the value for the change_ps1 field (defaults to true).
    pub fn change_ps1(&self) -> bool {
        self.shell.change_ps1.unwrap_or(true)
    }

    /// Retrieve the value for the auth_file field.
    pub fn authentication_override_file(&self) -> Option<&PathBuf> {
        self.authentication_override_file.as_ref()
    }

    /// Returns the global channel configuration.
    ///
    /// This roots the channel configuration to the current directory. When
    /// working with a project though the channel configuration should be rooted
    /// in the project directory.
    pub fn global_channel_config(&self) -> &ChannelConfig {
        &self.channel_config
    }

    pub fn repodata_config(&self) -> &RepodataConfig {
        &self.repodata_config
    }

    pub fn pypi_config(&self) -> &PyPIConfig {
        &self.pypi_config
    }

    pub fn mirror_map(&self) -> &std::collections::HashMap<Url, Vec<Url>> {
        &self.mirrors
    }

    /// Retrieve the value for the target_environments_directory field.
    pub fn detached_environments(&self) -> DetachedEnvironments {
        self.detached_environments.clone().unwrap_or_default()
    }

    pub fn force_activate(&self) -> bool {
        self.shell.force_activate.unwrap_or(false)
    }

    pub fn experimental_activation_cache_usage(&self) -> bool {
        self.experimental.use_environment_activation_cache()
    }

    /// Retrieve the value for the max_concurrent_solves field.
    pub fn max_concurrent_solves(&self) -> usize {
        self.concurrency.solves
    }

    /// Retrieve the value for the network_requests field.
    pub fn max_concurrent_downloads(&self) -> usize {
        self.concurrency.downloads
    }

    /// The platform to use to install tools.
    pub fn tool_platform(&self) -> Platform {
        self.tool_platform.unwrap_or(Platform::current())
    }

    pub fn get_proxies(&self) -> reqwest::Result<Vec<Proxy>> {
        if (self.proxy_config.https.is_none() && self.proxy_config.http.is_none())
            || *USE_PROXY_FROM_ENV
        {
            return Ok(vec![]);
        }

        let config_no_proxy =
            Some(self.proxy_config.non_proxy_hosts.iter().join(",")).filter(|v| !v.is_empty());

        let mut result: Vec<Proxy> = Vec::new();
        let config_no_proxy: Option<NoProxy> =
            config_no_proxy.as_deref().and_then(NoProxy::from_string);

        if self.proxy_config.https == self.proxy_config.http {
            result.push(
                Proxy::all(
                    self.proxy_config
                        .https
                        .as_ref()
                        .expect("must be some")
                        .as_str(),
                )?
                .no_proxy(config_no_proxy),
            );
        } else {
            if let Some(url) = &self.proxy_config.http {
                result.push(Proxy::http(url.as_str())?.no_proxy(config_no_proxy.clone()));
            }
            if let Some(url) = &self.proxy_config.https {
                result.push(Proxy::https(url.as_str())?.no_proxy(config_no_proxy));
            }
        }

        Ok(result)
    }

    /// Modify this config with the given key and value
    ///
    /// # Note
    ///
    /// It is required to call `save()` to persist the changes.
    pub fn set(&mut self, key: &str, value: Option<String>) -> miette::Result<()> {
        let show_supported_keys =
            || format!("Supported keys:\n\t{}", self.get_keys().join(",\n\t"));
        let err = miette::miette!(
            "Unknown key: {}\n{}",
            console::style(key).red(),
            show_supported_keys()
        );

        match key {
            "default-channels" => {
                self.default_channels = value
                    .map(|v| serde_json::de::from_str(&v))
                    .transpose()
                    .into_diagnostic()?
                    .unwrap_or_default();
            }
            "authentication-override-file" => {
                self.authentication_override_file = value.map(PathBuf::from);
            }
            "tls-no-verify" => {
                self.tls_no_verify = value.map(|v| v.parse()).transpose().into_diagnostic()?;
            }
            "tls-root-certs" => {
                self.tls_root_certs = value
                    .map(|v| TlsRootCerts::from_str(v.as_str()))
                    .transpose()
                    .into_diagnostic()?;
            }
            "mirrors" => {
                self.mirrors = value
                    .map(|v| serde_json::de::from_str(&v))
                    .transpose()
                    .into_diagnostic()?
                    .unwrap_or_default();
            }
            "detached-environments" => {
                self.detached_environments = value
                    .map(|v| {
                        Ok::<_, miette::Report>(match v.as_str() {
                            "true" => DetachedEnvironments::Boolean(true),
                            "false" => DetachedEnvironments::Boolean(false),
                            _ => DetachedEnvironments::Path(PathBuf::from(v)).resolve_path()?,
                        })
                    })
                    .transpose()?;
            }
            "pinning-strategy" => {
                self.pinning_strategy = value
                    .map(|v| PinningStrategy::from_str(v.as_str()))
                    .transpose()
                    .into_diagnostic()?
            }
            "change-ps1" => {
                return Err(miette::miette!(
                    "The `change-ps1` field is deprecated. Please use the `shell.change-ps1` field instead."
                ));
            }
            "force-activate" => {
                return Err(miette::miette!(
                    "The `force-activate` field is deprecated. Please use the `shell.force-activate` field instead."
                ));
            }
            "tool-platform" => {
                self.tool_platform = value
                    .as_deref()
                    .map(Platform::from_str)
                    .transpose()
                    .into_diagnostic()?;
            }
            key if key.starts_with("repodata-config") => {
                if key == "repodata-config" {
                    self.repodata_config = value
                        .map(|v| serde_json::de::from_str(&v))
                        .transpose()
                        .into_diagnostic()?
                        .unwrap_or_default();
                    return Ok(());
                } else if !key.starts_with("repodata-config.") {
                    return Err(err);
                }

                let subkey = key.strip_prefix("repodata-config.").unwrap();
                match subkey {
                    "disable-jlap" => {
                        self.repodata_config.default.disable_jlap =
                            value.map(|v| v.parse()).transpose().into_diagnostic()?;
                    }
                    "disable-bzip2" => {
                        self.repodata_config.default.disable_bzip2 =
                            value.map(|v| v.parse()).transpose().into_diagnostic()?;
                    }
                    "disable-zstd" => {
                        self.repodata_config.default.disable_zstd =
                            value.map(|v| v.parse()).transpose().into_diagnostic()?;
                    }
                    "disable-sharded" => {
                        self.repodata_config.default.disable_sharded =
                            value.map(|v| v.parse()).transpose().into_diagnostic()?;
                    }
                    _ => return Err(err),
                }
            }
            key if key.starts_with("pypi-config") => {
                if key == "pypi-config" {
                    if let Some(value) = value {
                        self.pypi_config = serde_json::de::from_str(&value).into_diagnostic()?;
                    } else {
                        self.pypi_config = PyPIConfig::default();
                    }
                    return Ok(());
                } else if !key.starts_with("pypi-config.") {
                    return Err(err);
                }

                let subkey = key.strip_prefix("pypi-config.").unwrap();
                match subkey {
                    "index-url" => {
                        self.pypi_config.index_url = value
                            .map(|v| Url::parse(&v))
                            .transpose()
                            .into_diagnostic()?;
                    }
                    "extra-index-urls" => {
                        self.pypi_config.extra_index_urls = value
                            .map(|v| serde_json::de::from_str(&v))
                            .transpose()
                            .into_diagnostic()?
                            .unwrap_or_default();
                    }
                    "keyring-provider" => {
                        self.pypi_config.keyring_provider = value
                            .map(|v| match v.as_str() {
                                "disabled" => Ok(KeyringProvider::Disabled),
                                "subprocess" => Ok(KeyringProvider::Subprocess),
                                _ => Err(miette::miette!("invalid keyring provider")),
                            })
                            .transpose()?;
                    }
                    "allow-insecure-host" => {
                        self.pypi_config.allow_insecure_host = value
                            .map(|v| serde_json::de::from_str(&v))
                            .transpose()
                            .into_diagnostic()?
                            .unwrap_or_default();
                    }
                    _ => return Err(err),
                }
            }
            key if key.starts_with("s3-options") => {
                if key == "s3-options" {
                    if let Some(value) = value {
                        self.s3_options = serde_json::de::from_str(&value).into_diagnostic()?;
                    } else {
                        return Err(miette!("s3-options requires a value"));
                    }
                    return Ok(());
                }
                let Some(subkey) = key.strip_prefix("s3-options.") else {
                    return Err(err);
                };
                if let Some((bucket, rest)) = subkey.split_once('.') {
                    if let Some(bucket_config) = self.s3_options.get_mut(bucket) {
                        match rest {
                            "endpoint-url" => {
                                if let Some(value) = value {
                                    bucket_config.endpoint_url =
                                        Url::parse(&value).into_diagnostic()?;
                                } else {
                                    return Err(miette!(
                                        "s3-options.{}.endpoint-url requires a value",
                                        bucket
                                    ));
                                }
                            }
                            "region" => {
                                if let Some(value) = value {
                                    bucket_config.region = value;
                                } else {
                                    return Err(miette!(
                                        "s3-options.{}.region requires a value",
                                        bucket
                                    ));
                                }
                            }
                            "force-path-style" => {
                                if let Some(value) = value {
                                    bucket_config.force_path_style =
                                        value.parse().into_diagnostic()?;
                                } else {
                                    return Err(miette!(
                                        "s3-options.{}.force-path-style requires a value",
                                        bucket
                                    ));
                                }
                            }
                            _ => return Err(err),
                        }
                    }
                } else {
                    let value = value.ok_or_else(|| miette!("s3-options requires a value"))?;
                    let s3_options: S3Options =
                        serde_json::de::from_str(&value).into_diagnostic()?;
                    self.s3_options.insert(subkey.to_string(), s3_options);
                }
            }
            key if key.starts_with(EXPERIMENTAL) => {
                if key == EXPERIMENTAL {
                    if let Some(value) = value {
                        self.experimental = serde_json::de::from_str(&value).into_diagnostic()?;
                    } else {
                        self.experimental = ExperimentalConfig::default();
                    }
                    return Ok(());
                } else if !key.starts_with(format!("{EXPERIMENTAL}.").as_str()) {
                    return Err(err);
                }

                let subkey = key
                    .strip_prefix(format!("{EXPERIMENTAL}.").as_str())
                    .unwrap();
                match subkey {
                    "use-environment-activation-cache" => {
                        self.experimental.use_environment_activation_cache =
                            value.map(|v| v.parse()).transpose().into_diagnostic()?;
                    }
                    _ => return Err(err),
                }
            }
            key if key.starts_with("concurrency") => {
                if key == "concurrency" {
                    if let Some(value) = value {
                        self.pypi_config = serde_json::de::from_str(&value).into_diagnostic()?;
                    } else {
                        self.pypi_config = PyPIConfig::default();
                    }
                    return Ok(());
                } else if !key.starts_with("concurrency.") {
                    return Err(err);
                }
                let subkey = key.strip_prefix("concurrency.").unwrap();
                match subkey {
                    "solves" => {
                        if let Some(value) = value {
                            self.concurrency.solves = value.parse().into_diagnostic()?;
                        } else {
                            return Err(miette!("'solves' requires a number value"));
                        }
                    }
                    "downloads" => {
                        if let Some(value) = value {
                            self.concurrency.downloads = value.parse().into_diagnostic()?;
                        } else {
                            return Err(miette!("'downloads' requires a number value"));
                        }
                    }
                    _ => return Err(err),
                }
            }
            key if key.starts_with("shell") => {
                if key == "shell" {
                    if let Some(value) = value {
                        self.shell = serde_json::de::from_str(&value).into_diagnostic()?;
                    } else {
                        self.shell = ShellConfig::default();
                    }
                    return Ok(());
                } else if !key.starts_with("shell.") {
                    return Err(err);
                }
                let subkey = key.strip_prefix("shell.").unwrap();
                match subkey {
                    "force-activate" => {
                        self.shell.force_activate =
                            value.map(|v| v.parse()).transpose().into_diagnostic()?;
                    }
                    "source-completion-scripts" => {
                        self.shell.source_completion_scripts =
                            value.map(|v| v.parse()).transpose().into_diagnostic()?;
                    }
                    "change-ps1" => {
                        self.shell.change_ps1 =
                            value.map(|v| v.parse()).transpose().into_diagnostic()?;
                    }
                    _ => return Err(err),
                }
            }
            key if key.starts_with("run-post-link-scripts") => {
                if let Some(value) = value {
                    self.run_post_link_scripts = Some(
                        value
                            .parse()
                            .into_diagnostic()
                            .wrap_err("failed to parse run-post-link-scripts")?,
                    );
                }
                return Ok(());
            }
            key if key.starts_with("proxy-config") => {
                if key == "proxy-config" {
                    if let Some(value) = value {
                        self.proxy_config = serde_json::de::from_str(&value).into_diagnostic()?;
                    } else {
                        self.proxy_config = ProxyConfig::default();
                    }
                    return Ok(());
                } else if !key.starts_with("proxy-config.") {
                    return Err(err);
                }

                let subkey = key.strip_prefix("proxy-config.").unwrap();
                match subkey {
                    "https" => {
                        self.proxy_config.https = value
                            .map(|v| Url::parse(&v))
                            .transpose()
                            .into_diagnostic()?;
                    }
                    "http" => {
                        self.proxy_config.http = value
                            .map(|v| Url::parse(&v))
                            .transpose()
                            .into_diagnostic()?;
                    }
                    "non-proxy-hosts" => {
                        self.proxy_config.non_proxy_hosts = value
                            .map(|v| serde_json::de::from_str(&v))
                            .transpose()
                            .into_diagnostic()?
                            .unwrap_or_default();
                    }
                    _ => return Err(err),
                }
            }
            _ => return Err(err),
        }

        Ok(())
    }

    /// Save the config to the given path.
    pub fn save(&self, to: &Path) -> miette::Result<()> {
        let contents = toml_edit::ser::to_string_pretty(&self).into_diagnostic()?;
        tracing::debug!("Saving config to: {}", to.display());

        let parent = to.parent().expect("config path should have a parent");
        fs_err::create_dir_all(parent)
            .into_diagnostic()
            .wrap_err(format!(
                "failed to create directories in '{}'",
                parent.display()
            ))?;
        fs_err::write(to, contents)
            .into_diagnostic()
            .wrap_err(format!("failed to write config to '{}'", to.display()))
    }

    /// Constructs a [`GatewayBuilder`] with preconfigured settings.
    pub fn gateway(&self) -> GatewayBuilder {
        // Determine the cache directory and fall back to sane defaults otherwise.
        let cache_dir = get_cache_dir().unwrap_or_else(|e| {
            tracing::error!("failed to determine repodata cache directory: {e}");
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("./"))
        });

        // Construct the gateway
        Gateway::builder()
            .with_cache_dir(cache_dir.join(consts::CONDA_REPODATA_CACHE_DIR))
            .with_channel_config(self.into())
            .with_max_concurrent_requests(self.max_concurrent_downloads())
    }

    pub fn compute_s3_config(&self) -> HashMap<String, s3_middleware::S3Config> {
        self.s3_options
            .clone()
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    s3_middleware::S3Config::Custom {
                        endpoint_url: v.endpoint_url.clone(),
                        region: v.region.clone(),
                        force_path_style: v.force_path_style,
                    },
                )
            })
            .collect()
    }

    /// Retrieve the value for the run_post_link_scripts field or default to
    /// false.
    pub fn run_post_link_scripts(&self) -> RunPostLinkScripts {
        self.run_post_link_scripts.clone().unwrap_or_default()
    }
}

/// Returns the path to the system-level pixi config file.
pub fn config_path_system() -> PathBuf {
    // TODO: the base_path for Windows is currently hardcoded, it should be
    // determined via the system API to support general volume label
    #[cfg(target_os = "windows")]
    let base_path = PathBuf::from("C:\\ProgramData");
    #[cfg(not(target_os = "windows"))]
    let base_path = PathBuf::from("/etc");

    base_path.join(consts::CONFIG_DIR).join(consts::CONFIG_FILE)
}

/// Returns the path(s) to the global pixi config file.
pub fn config_path_global() -> Vec<PathBuf> {
    vec![
        // On macos, add the XDG_CONFIG_HOME directory as well, although it's not a standard and
        // not set by default.
        #[cfg(target_os = "macos")]
        std::env::var("XDG_CONFIG_HOME").ok().map(|d| {
            PathBuf::from(d)
                .join(consts::CONFIG_DIR)
                .join(consts::CONFIG_FILE)
        }),
        dirs::config_dir().map(|d| d.join(consts::CONFIG_DIR).join(consts::CONFIG_FILE)),
        pixi_home().map(|d| d.join(consts::CONFIG_FILE)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[test]
    fn test_config_parse() {
        let toml = format!(
            r#"default-channels = ["conda-forge"]
tls-no-verify = true
tls-root-certs = "native"
detached-environments = "{}"
pinning-strategy = "no-pin"
concurrency.solves = 5
UNUSED = "unused"
        "#,
            env!("CARGO_MANIFEST_DIR").replace('\\', "\\\\").as_str()
        );
        let (config, unused) = Config::from_toml(toml.as_str(), None).unwrap();
        assert_eq!(
            config.default_channels,
            vec![NamedChannelOrUrl::from_str("conda-forge").unwrap()]
        );
        assert_eq!(config.tls_no_verify, Some(true));
        assert_eq!(config.tls_root_certs, Some(TlsRootCerts::Native));
        assert_eq!(
            config.detached_environments().path().unwrap(),
            Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")))
        );
        assert_eq!(config.max_concurrent_solves(), 5);
        assert!(unused.contains("UNUSED"));

        let toml = r"detached-environments = true";
        let (config, _) = Config::from_toml(toml, None).unwrap();
        assert_eq!(
            config.detached_environments().path().unwrap().unwrap(),
            get_cache_dir()
                .unwrap()
                .join(consts::ENVIRONMENTS_DIR)
                .as_path()
        );
    }

    #[rstest]
    #[case("semver", PinningStrategy::Semver)]
    #[case("major", PinningStrategy::Major)]
    #[case("minor", PinningStrategy::Minor)]
    #[case("exact-version", PinningStrategy::ExactVersion)]
    #[case("latest-up", PinningStrategy::LatestUp)]
    #[case("no-pin", PinningStrategy::NoPin)]
    fn test_config_parse_pinning_strategy(#[case] input: &str, #[case] expected: PinningStrategy) {
        let toml = format!("pinning-strategy = \"{input}\"");
        let (config, _) = Config::from_toml(&toml, None).unwrap();
        assert_eq!(config.pinning_strategy, Some(expected));
    }

    /// Assert that usage of `~` in `detached_environments` is correctly expanded
    /// to the absolute path to the home directory.
    #[test]
    fn test_detached_environments_resolve_home_dir() {
        let home_dir = dirs::home_dir().expect("Failed to resolve home directory");
        let toml = r#"detached-environments = "~/my/envs""#;

        let expected_detached_envs_path = home_dir.join("my/envs");

        let (config, _) = Config::from_toml(toml, None).unwrap();
        let actual_detached_envs_path = config.detached_environments().path().unwrap().unwrap();

        assert_eq!(actual_detached_envs_path, expected_detached_envs_path);
    }

    /// Assert that an absolute path in `detached_environments` is preserved.
    #[test]
    fn test_detached_environments_abs_path() {
        let toml = r#"detached-environments = "/home/me/envs""#;

        let (config, _) = Config::from_toml(toml, None).unwrap();
        let actual_detached_envs_path = config.detached_environments().path().unwrap().unwrap();
        let expected_detached_envs_path = PathBuf::from("/home/me/envs");
        assert_eq!(actual_detached_envs_path, expected_detached_envs_path);
    }

    /// Assert that en error is thrown if a relative path is used in `detached_environments`.
    #[test]
    fn test_detached_environments_relative_path() {
        let toml = r#"detached-environments = "./relative_path/""#;

        let (config, _) = Config::from_toml(toml, None).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        let error_message = result.unwrap_err().to_string();
        assert_eq!(
            error_message,
            "The `detached-environments` path must be an absolute path: ./relative_path/"
        );
    }

    /// Assert that a boolean in `detached_environments` is preserved.
    #[test]
    fn test_detached_environments_bool() {
        let toml = r#"detached-environments = true"#;

        let (config, _) = Config::from_toml(toml, None).unwrap();

        assert_eq!(
            config.detached_environments(),
            DetachedEnvironments::Boolean(true)
        );
    }

    #[test]
    fn test_config_from_cli() {
        // Test with all CLI options enabled
        let cli = ConfigCli {
            tls_no_verify: true,
            tls_root_certs: Some(TlsRootCerts::Native),
            auth_file: None,
            pypi_keyring_provider: Some(KeyringProvider::Subprocess),
            concurrent_solves: Some(8),
            concurrent_downloads: Some(100),
            run_post_link_scripts: true,
            use_environment_activation_cache: true,
            pinning_strategy: Some(PinningStrategy::Semver),
        };
        let config = Config::from(cli);
        assert_eq!(config.tls_no_verify, Some(true));
        assert_eq!(config.tls_root_certs, Some(TlsRootCerts::Native));
        assert_eq!(
            config.pypi_config().keyring_provider,
            Some(KeyringProvider::Subprocess)
        );
        assert_eq!(config.concurrency.solves, 8);
        assert_eq!(config.concurrency.downloads, 100);
        assert_eq!(
            config.run_post_link_scripts,
            Some(RunPostLinkScripts::Insecure)
        );
        assert_eq!(
            config.experimental.use_environment_activation_cache,
            Some(true)
        );
        assert_eq!(config.pinning_strategy, Some(PinningStrategy::Semver));

        let cli = ConfigCli {
            tls_no_verify: false,
            tls_root_certs: None,
            auth_file: Some(PathBuf::from("path.json")),
            pypi_keyring_provider: None,
            concurrent_solves: None,
            concurrent_downloads: None,
            run_post_link_scripts: false,
            use_environment_activation_cache: false,
            pinning_strategy: None,
        };

        let config = Config::from(cli);
        assert_eq!(config.tls_no_verify, None);
        assert_eq!(config.tls_root_certs, None);
        assert_eq!(
            config.authentication_override_file,
            Some(PathBuf::from("path.json"))
        );
        assert_eq!(config.run_post_link_scripts, None);
        assert_eq!(config.experimental.use_environment_activation_cache, None);
        assert_eq!(config.pinning_strategy, None);
    }

    #[test]
    fn test_pypi_config_parse() {
        let toml = r#"
            [pypi-config]
            index-url = "https://pypi.org/simple"
            extra-index-urls = ["https://pypi.org/simple2"]
            keyring-provider = "subprocess"
        "#;
        let (config, _) = Config::from_toml(toml, None).unwrap();
        assert_eq!(
            config.pypi_config().index_url,
            Some(Url::parse("https://pypi.org/simple").unwrap())
        );
        assert_eq!(config.pypi_config().extra_index_urls.len(), 1);
        assert_eq!(
            config.pypi_config().keyring_provider,
            Some(KeyringProvider::Subprocess)
        );
    }

    #[test]
    fn test_pypi_config_allow_insecure_host() {
        let toml = r#"
            [pypi-config]
            index-url = "https://pypi.org/simple"
            extra-index-urls = ["https://pypi.org/simple2"]
            keyring-provider = "subprocess"
            allow-insecure-host = ["https://localhost:1234", "*"]
        "#;
        let (config, _) = Config::from_toml(toml, None).unwrap();
        assert_eq!(
            config.pypi_config().allow_insecure_host,
            vec!["https://localhost:1234", "*",]
        );
    }

    #[test]
    fn test_s3_options_parse() {
        let toml = r#"
            [s3-options.bucket1]
            endpoint-url = "https://my-s3-host"
            region = "us-east-1"
            force-path-style = false
        "#;
        let (config, _) = Config::from_toml(toml, None).unwrap();
        let s3_options = config.s3_options;
        assert_eq!(
            s3_options["bucket1"].endpoint_url,
            Url::parse("https://my-s3-host").unwrap()
        );
        assert_eq!(s3_options["bucket1"].region, "us-east-1");
        assert!(!s3_options["bucket1"].force_path_style);
    }

    #[test]
    fn test_s3_options_invalid_config() {
        let toml = r#"
            [s3-options.bucket1]
            endpoint-url = "https://my-s3-host"
            region = "us-east-1"
            # force-path-style = false
        "#;
        let result = Config::from_toml(toml, None);
        assert!(result.is_err());
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("missing field `force-path-style`")
        );
    }

    #[test]
    fn test_default_config() {
        let config = Config::default();
        // This depends on the system so it's hard to test.
        assert!(config.concurrency.solves > 0);
        assert_eq!(config.concurrency.downloads, 50);
    }

    #[test]
    fn test_config_merge_priority() {
        // If I set every config key, ensure that `other wins`
        let mut config = Config::default();
        let other = Config {
            default_channels: vec![NamedChannelOrUrl::from_str("conda-forge").unwrap()],
            channel_config: ChannelConfig::default_with_root_dir(PathBuf::from("/root/dir")),
            tls_no_verify: Some(true),
            tls_root_certs: Some(TlsRootCerts::Native),
            detached_environments: Some(DetachedEnvironments::Path(PathBuf::from("/path/to/envs"))),
            concurrency: ConcurrencyConfig {
                solves: 5,
                ..ConcurrencyConfig::default()
            },
            authentication_override_file: Some(PathBuf::default()),
            mirrors: HashMap::from([(
                Url::parse("https://conda.anaconda.org/conda-forge").unwrap(),
                Vec::default(),
            )]),
            pinning_strategy: Some(PinningStrategy::NoPin),
            experimental: ExperimentalConfig {
                use_environment_activation_cache: Some(true),
            },
            loaded_from: Vec::from([PathBuf::from_str("test").unwrap()]),
            shell: ShellConfig {
                force_activate: Some(true),
                source_completion_scripts: None,
                change_ps1: Some(false),
            },
            pypi_config: PyPIConfig {
                allow_insecure_host: Vec::from(["test".to_string()]),
                extra_index_urls: Vec::from([
                    Url::parse("https://conda.anaconda.org/conda-forge").unwrap()
                ]),
                index_url: Some(Url::parse("https://conda.anaconda.org/conda-forge").unwrap()),
                keyring_provider: Some(KeyringProvider::Subprocess),
            },
            s3_options: HashMap::from([(
                "bucket1".into(),
                S3Options {
                    endpoint_url: Url::parse("https://my-s3-host").unwrap(),
                    region: "us-east-1".to_string(),
                    force_path_style: false,
                },
            )]),
            repodata_config: RepodataConfig {
                default: RepodataChannelConfig {
                    disable_bzip2: Some(true),
                    disable_jlap: Some(true),
                    disable_sharded: Some(true),
                    disable_zstd: Some(true),
                },
                per_channel: HashMap::from([(
                    Url::parse("https://conda.anaconda.org/conda-forge").unwrap(),
                    RepodataChannelConfig::default(),
                )]),
            },
            run_post_link_scripts: Some(RunPostLinkScripts::Insecure),
            proxy_config: ProxyConfig::default(),
            build: BuildConfig::default(),
            tool_platform: None,
            // Deprecated keys
            change_ps1: None,
            force_activate: None,
        };
        let original_other = other.clone();
        config = config.merge_config(other);
        assert_eq!(config, original_other);
    }
    #[test]
    fn test_config_merge_multiple() {
        let mut config = Config::default();
        let other = Config {
            default_channels: vec![NamedChannelOrUrl::from_str("conda-forge").unwrap()],
            channel_config: ChannelConfig::default_with_root_dir(PathBuf::from("/root/dir")),
            tls_no_verify: Some(true),
            detached_environments: Some(DetachedEnvironments::Path(PathBuf::from("/path/to/envs"))),
            concurrency: ConcurrencyConfig {
                solves: 5,
                ..ConcurrencyConfig::default()
            },
            s3_options: HashMap::from([
                (
                    "bucket1".into(),
                    S3Options {
                        endpoint_url: Url::parse("https://my-s3-host").unwrap(),
                        region: "us-east-1".to_string(),
                        force_path_style: false,
                    },
                ),
                (
                    "bucket2".into(),
                    S3Options {
                        endpoint_url: Url::parse("https://my-s3-host").unwrap(),
                        region: "us-east-1".to_string(),
                        force_path_style: false,
                    },
                ),
            ]),
            ..Default::default()
        };
        config = config.merge_config(other);
        assert_eq!(
            config.default_channels,
            vec![NamedChannelOrUrl::from_str("conda-forge").unwrap()]
        );
        assert_eq!(config.tls_no_verify, Some(true));
        assert_eq!(
            config.detached_environments().path().unwrap(),
            Some(PathBuf::from("/path/to/envs"))
        );
        assert!(config.s3_options.contains_key("bucket1"));

        let other2 = Config {
            default_channels: vec![NamedChannelOrUrl::from_str("channel").unwrap()],
            channel_config: ChannelConfig::default_with_root_dir(PathBuf::from("/root/dir2")),
            tls_no_verify: Some(false),
            detached_environments: Some(DetachedEnvironments::Path(PathBuf::from(
                "/path/to/envs2",
            ))),
            s3_options: HashMap::from([(
                "bucket2".into(),
                S3Options {
                    endpoint_url: Url::parse("https://my-new-s3-host").unwrap(),
                    region: "us-east-1".to_string(),
                    force_path_style: false,
                },
            )]),
            ..Default::default()
        };

        config = config.merge_config(other2);
        assert_eq!(
            config.default_channels,
            vec![NamedChannelOrUrl::from_str("channel").unwrap()]
        );
        assert_eq!(config.tls_no_verify, Some(false));
        assert_eq!(
            config.detached_environments().path().unwrap(),
            Some(PathBuf::from("/path/to/envs2"))
        );
        assert_eq!(config.max_concurrent_solves(), 5);
        assert!(config.s3_options.contains_key("bucket1"));
        assert!(config.s3_options.contains_key("bucket2"));
        assert!(
            config.s3_options["bucket2"]
                .endpoint_url
                .to_string()
                .contains("my-new-s3-host")
        );

        let d = Path::new(&env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("config");

        let config_1 = Config::from_path(&d.join("config_1.toml")).unwrap();
        let config_2 = Config::from_path(&d.join("config_2.toml")).unwrap();
        let config_2 = Config {
            channel_config: ChannelConfig::default_with_root_dir(PathBuf::from("/root/dir")),
            detached_environments: Some(DetachedEnvironments::Boolean(true)),
            ..config_2
        };

        let mut merged = config_1.clone();
        merged = merged.merge_config(config_2);
        assert!(merged.s3_options.contains_key("bucket1"));

        let debug = format!("{merged:#?}");
        let debug = debug.replace("\\\\", "/");
        // replace the path with a placeholder
        let debug = debug.replace(&d.to_str().unwrap().replace('\\', "/"), "path");
        insta::assert_snapshot!(debug);
    }

    #[test]
    fn test_parse_kebab_and_snake_case() {
        let toml = r#"
            default_channels = ["conda-forge"]
            change_ps1 = true
            tls_no_verify = false
            authentication_override_file = "/path/to/your/override.json"
            [mirrors]
            "https://conda.anaconda.org/conda-forge" = [
                "https://prefix.dev/conda-forge"
            ]
            [repodata_config]
            disable_jlap = true
            disable_bzip2 = true
            disable_zstd = true
        "#;
        let (config, _) = Config::from_toml(toml, None).unwrap();
        assert_eq!(
            config.default_channels,
            vec![NamedChannelOrUrl::from_str("conda-forge").unwrap()]
        );
        assert_eq!(config.tls_no_verify, Some(false));
        assert_eq!(
            config.authentication_override_file,
            Some(PathBuf::from("/path/to/your/override.json"))
        );
        assert_eq!(config.change_ps1, Some(true));
        assert_eq!(
            config
                .mirrors
                .get(&Url::parse("https://conda.anaconda.org/conda-forge").unwrap()),
            Some(&vec![Url::parse("https://prefix.dev/conda-forge").unwrap()])
        );
        let repodata_config = config.repodata_config;
        assert_eq!(repodata_config.default.disable_jlap, Some(true));
        assert_eq!(repodata_config.default.disable_bzip2, Some(true));
        assert_eq!(repodata_config.default.disable_zstd, Some(true));
        assert_eq!(repodata_config.default.disable_sharded, None);
        // See if the toml parses in kebab-case
        let toml = r#"
            default-channels = ["conda-forge"]
            change-ps1 = true
            tls-no-verify = false
            authentication-override-file = "/path/to/your/override.json"
            [mirrors]
            "https://conda.anaconda.org/conda-forge" = [
                "https://prefix.dev/conda-forge"
            ]
            [repodata-config]
            disable-jlap = true
            disable-bzip2 = true
            disable-zstd = true
            disable-sharded = true
        "#;
        Config::from_toml(toml, None).unwrap();
    }

    #[test]
    fn test_alter_config() {
        let mut config = Config::default();
        config
            .set("default-channels", Some(r#"["conda-forge"]"#.to_string()))
            .unwrap();
        assert_eq!(
            config.default_channels,
            vec![NamedChannelOrUrl::from_str("conda-forge").unwrap()]
        );

        config
            .set("tls-no-verify", Some("true".to_string()))
            .unwrap();
        assert_eq!(config.tls_no_verify, Some(true));

        config
            .set(
                "authentication-override-file",
                Some("/path/to/your/override.json".to_string()),
            )
            .unwrap();
        assert_eq!(
            config.authentication_override_file,
            Some(PathBuf::from("/path/to/your/override.json"))
        );

        config
            .set("detached-environments", Some("true".to_string()))
            .unwrap();
        assert_eq!(
            config.detached_environments().path().unwrap().unwrap(),
            get_cache_dir()
                .unwrap()
                .join(consts::ENVIRONMENTS_DIR)
                .as_path()
        );

        config
            .set("detached-environments", Some("/path/to/envs".to_string()))
            .unwrap();
        assert_eq!(
            config.detached_environments().path().unwrap(),
            Some(PathBuf::from("/path/to/envs"))
        );

        config
            .set("mirrors", Some(r#"{"https://conda.anaconda.org/conda-forge": ["https://prefix.dev/conda-forge"]}"#.to_string()))
            .unwrap();
        assert_eq!(
            config
                .mirrors
                .get(&Url::parse("https://conda.anaconda.org/conda-forge").unwrap()),
            Some(&vec![Url::parse("https://prefix.dev/conda-forge").unwrap()])
        );

        config
            .set("repodata-config.disable-jlap", Some("true".to_string()))
            .unwrap();
        let repodata_config = config.repodata_config();
        assert_eq!(repodata_config.default.disable_jlap, Some(true));

        config
            .set(
                "pypi-config.index-url",
                Some("https://pypi.org/simple".to_string()),
            )
            .unwrap();
        assert_eq!(
            config.pypi_config().index_url,
            Some(Url::parse("https://pypi.org/simple").unwrap())
        );

        config
            .set(
                "pypi-config.extra-index-urls",
                Some(r#"["https://pypi.org/simple2"]"#.to_string()),
            )
            .unwrap();
        assert!(config.pypi_config().extra_index_urls.len() == 1);

        config
            .set(
                "pypi-config.keyring-provider",
                Some("subprocess".to_string()),
            )
            .unwrap();
        assert_eq!(
            config.pypi_config().keyring_provider,
            Some(KeyringProvider::Subprocess)
        );

        let deprecated = config.set("change-ps1", None);
        assert!(deprecated.is_err());
        assert!(deprecated.unwrap_err().to_string().contains("deprecated"));

        config.set("shell.change-ps1", None).unwrap();
        assert_eq!(config.change_ps1, None);

        config
            .set("concurrency.solves", Some("10".to_string()))
            .unwrap();
        assert_eq!(config.max_concurrent_solves(), 10);
        config
            .set("concurrency.solves", Some("1".to_string()))
            .unwrap();

        config
            .set("concurrency.downloads", Some("10".to_string()))
            .unwrap();
        assert_eq!(config.max_concurrent_downloads(), 10);
        config
            .set("concurrency.downloads", Some("1".to_string()))
            .unwrap();

        assert_eq!(config.max_concurrent_downloads(), 1);

        config.set("s3-options.my-bucket", Some(r#"{"endpoint-url": "http://localhost:9000", "force-path-style": true, "region": "auto"}"#.to_string())).unwrap();
        let s3_options = config.s3_options.get("my-bucket").unwrap();
        assert!(
            s3_options
                .endpoint_url
                .to_string()
                .contains("http://localhost:9000")
        );
        assert!(s3_options.force_path_style);
        assert_eq!(s3_options.region, "auto");

        // Test tool-platform
        config
            .set("tool-platform", Some("linux-64".to_string()))
            .unwrap();
        assert_eq!(config.tool_platform, Some(Platform::Linux64));

        // Test run-post-link-scripts
        config
            .set("run-post-link-scripts", Some("insecure".to_string()))
            .unwrap();
        assert_eq!(
            config.run_post_link_scripts,
            Some(RunPostLinkScripts::Insecure)
        );

        // Test shell.force-activate
        config
            .set("shell.force-activate", Some("true".to_string()))
            .unwrap();
        assert_eq!(config.shell.force_activate, Some(true));

        // Test shell.source-completion-scripts
        config
            .set("shell.source-completion-scripts", Some("false".to_string()))
            .unwrap();
        assert_eq!(config.shell.source_completion_scripts, Some(false));

        // Test experimental.use-environment-activation-cache
        config
            .set(
                "experimental.use-environment-activation-cache",
                Some("true".to_string()),
            )
            .unwrap();
        assert_eq!(
            config.experimental.use_environment_activation_cache,
            Some(true)
        );

        // Test more repodata-config options
        config
            .set("repodata-config.disable-bzip2", Some("true".to_string()))
            .unwrap();
        let repodata_config = config.repodata_config();
        assert_eq!(repodata_config.default.disable_bzip2, Some(true));

        config
            .set("repodata-config.disable-zstd", Some("false".to_string()))
            .unwrap();
        let repodata_config = config.repodata_config();
        assert_eq!(repodata_config.default.disable_zstd, Some(false));

        config
            .set("repodata-config.disable-sharded", Some("true".to_string()))
            .unwrap();
        let repodata_config = config.repodata_config();
        assert_eq!(repodata_config.default.disable_sharded, Some(true));

        // Test pypi-config.allow-insecure-host
        config
            .set(
                "pypi-config.allow-insecure-host",
                Some(r#"["pypi.example.com"]"#.to_string()),
            )
            .unwrap();
        assert_eq!(config.pypi_config().allow_insecure_host.len(), 1);

        // Test proxy-config
        config
            .set(
                "proxy-config.http",
                Some("http://proxy.example.com:8080".to_string()),
            )
            .unwrap();
        assert_eq!(
            config.proxy_config.http,
            Some(Url::parse("http://proxy.example.com:8080").unwrap())
        );

        config
            .set(
                "proxy-config.https",
                Some("https://proxy.example.com:8080".to_string()),
            )
            .unwrap();
        assert_eq!(
            config.proxy_config.https,
            Some(Url::parse("https://proxy.example.com:8080").unwrap())
        );

        config
            .set(
                "proxy-config.non-proxy-hosts",
                Some(r#"["localhost", "127.0.0.1"]"#.to_string()),
            )
            .unwrap();
        assert_eq!(config.proxy_config.non_proxy_hosts.len(), 2);

        // Test s3-options with individual keys
        config
            .set(
                "s3-options.test-bucket.endpoint-url",
                Some("http://localhost:9000".to_string()),
            )
            .unwrap();
        config
            .set(
                "s3-options.test-bucket.region",
                Some("us-east-1".to_string()),
            )
            .unwrap();
        config
            .set(
                "s3-options.test-bucket.force-path-style",
                Some("false".to_string()),
            )
            .unwrap();

        // Test concurrency configuration
        config
            .set("concurrency.solves", Some("5".to_string()))
            .unwrap();
        assert_eq!(config.concurrency.solves, 5);

        config
            .set("concurrency.downloads", Some("25".to_string()))
            .unwrap();
        assert_eq!(config.concurrency.downloads, 25);

        // Test max-concurrent-solves (legacy accessor)
        assert_eq!(config.max_concurrent_solves(), 5);
        assert_eq!(config.max_concurrent_downloads(), 25);

        // Test tls-no-verify
        config
            .set("tls-no-verify", Some("true".to_string()))
            .unwrap();
        assert_eq!(config.tls_no_verify, Some(true));

        // Test tls-root-certs
        config
            .set("tls-root-certs", Some("native".to_string()))
            .unwrap();
        assert_eq!(config.tls_root_certs, Some(TlsRootCerts::Native));

        // Test mirrors
        config
            .set(
                "mirrors",
                Some(r#"{"https://conda.anaconda.org/conda-forge": ["https://prefix.dev/conda-forge"]}"#.to_string()),
            )
            .unwrap();
        assert_eq!(config.mirrors.len(), 1);

        // Test detached-environments
        config
            .set("detached-environments", Some("/custom/path".to_string()))
            .unwrap();
        assert!(matches!(
            config.detached_environments,
            Some(DetachedEnvironments::Path(_))
        ));

        // Test pinning-strategy
        config
            .set("pinning-strategy", Some("semver".to_string()))
            .unwrap();
        assert_eq!(config.pinning_strategy, Some(PinningStrategy::Semver));

        config.set("unknown-key", None).unwrap_err();
    }

    #[rstest]
    #[case("pinning-strategy", None, None)]
    #[case("pinning-strategy", Some("semver".to_string()), Some(PinningStrategy::Semver))]
    #[case("pinning-strategy", Some("no-pin".to_string()), Some(PinningStrategy::NoPin))]
    #[case("pinning-strategy", Some("exact-version".to_string()), Some(PinningStrategy::ExactVersion))]
    #[case("pinning-strategy", Some("latest-up".to_string()), Some(PinningStrategy::LatestUp))]
    #[case("pinning-strategy", Some("major".to_string()), Some(PinningStrategy::Major))]
    #[case("pinning-strategy", Some("minor".to_string()), Some(PinningStrategy::Minor))]
    fn test_set_pinning_strategy(
        #[case] key: &str,
        #[case] value: Option<String>,
        #[case] expected: Option<PinningStrategy>,
    ) {
        let mut config = Config::default();
        config.set(key, value).unwrap();
        assert_eq!(config.pinning_strategy, expected);
    }

    #[test]
    fn test_version_constraints() {
        let versions = vec![
            vec!["1.2.3"],
            vec!["0.0.0"],
            vec!["1!1"],
            vec!["1!0.0.0"],
            vec!["1.2.3a1"],
            vec!["1.2.3a1", "1.2.3"],
            vec!["1.2.3", "1.2.3a1", "1.2.3b1", "1.2.3rc1", "2", "10000"],
            vec!["1.2.0", "1.3.0"],
            vec!["0.2.0", "0.3.0"],
            vec!["0.2.0", "1.3.0"],
            vec!["1.2"],
            vec!["1.2", "2"],
            vec!["1.2", "1!2.0"],
            vec!["24.2"],
        ];

        // We could use `strum` for this, but it requires another dependency
        let strategies = vec![
            PinningStrategy::Semver,
            PinningStrategy::Major,
            PinningStrategy::Minor,
            PinningStrategy::ExactVersion,
            PinningStrategy::LatestUp,
            PinningStrategy::NoPin,
        ];

        let results = strategies
            .into_iter()
            .map(|strategy| {
                let constraints: Vec<String> = versions
                    .iter()
                    .map(|v| {
                        let constraint = strategy
                            .determine_version_constraint(
                                v.clone()
                                    .into_iter()
                                    .map(|v| v.parse().unwrap())
                                    .collect::<Vec<Version>>()
                                    .as_slice(),
                            )
                            .unwrap()
                            .to_string();
                        format!("{} from {}", constraint, v.join(", "))
                    })
                    .collect();
                format!(
                    "### Strategy: '{:?}'\n{}\n",
                    strategy,
                    constraints.join("\n")
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        insta::assert_snapshot!(results);
    }

    #[test]
    fn test_repodata_config() {
        let toml = r#"
            [repodata-config]
            disable-jlap = true
            disable-bzip2 = true
            disable-zstd = true
            disable-sharded = true

            [repodata-config."https://prefix.dev/conda-forge"]
            disable-jlap = false
            disable-bzip2 = false
            disable-zstd = false
            disable-sharded = false

            [repodata-config."https://conda.anaconda.org/conda-forge"]
            disable-jlap = false
            disable-bzip2 = false
            disable-zstd = false
        "#;
        let (config, _) = Config::from_toml(toml, None).unwrap();
        let repodata_config = config.repodata_config();
        assert_eq!(repodata_config.default.disable_jlap, Some(true));
        assert_eq!(repodata_config.default.disable_bzip2, Some(true));
        assert_eq!(repodata_config.default.disable_zstd, Some(true));
        assert_eq!(repodata_config.default.disable_sharded, Some(true));

        let per_channel = repodata_config.clone().per_channel;
        assert_eq!(per_channel.len(), 2);

        let prefix_config = per_channel
            .get(&Url::from_str("https://prefix.dev/conda-forge").unwrap())
            .unwrap();
        assert_eq!(prefix_config.disable_jlap, Some(false));
        assert_eq!(prefix_config.disable_bzip2, Some(false));
        assert_eq!(prefix_config.disable_zstd, Some(false));
        assert_eq!(prefix_config.disable_sharded, Some(false));

        let anaconda_config = per_channel
            .get(&Url::from_str("https://conda.anaconda.org/conda-forge").unwrap())
            .unwrap();
        assert_eq!(anaconda_config.disable_jlap, Some(false));
        assert_eq!(anaconda_config.disable_bzip2, Some(false));
        assert_eq!(anaconda_config.disable_zstd, Some(false));
        assert_eq!(anaconda_config.disable_sharded, None);
    }

    #[test]
    fn test_proxy_config_parse() {
        let toml = r#"
            [proxy-config]
            https = "http://proxy-for-https"
            http = "http://proxy-for-http"
            non-proxy-hosts = [ "a.com" ]
        "#;
        let (config, _) = Config::from_toml(toml, None).unwrap();
        assert_eq!(
            config.proxy_config.https,
            Some(Url::parse("http://proxy-for-https").unwrap())
        );
        assert_eq!(
            config.proxy_config.http,
            Some(Url::parse("http://proxy-for-http").unwrap())
        );
        assert_eq!(config.proxy_config.non_proxy_hosts.len(), 1);
        assert_eq!(config.proxy_config.non_proxy_hosts[0], "a.com");
    }

    use std::str::FromStr;

    use rattler_conda_types::{compression_level::CompressionLevel, package::CondaArchiveType};

    use super::PackageFormatAndCompression;

    #[test]
    fn test_parse_packaging() {
        let package_format = PackageFormatAndCompression::from_str("tar-bz2").unwrap();
        assert_eq!(
            package_format,
            PackageFormatAndCompression {
                archive_type: CondaArchiveType::TarBz2,
                compression_level: CompressionLevel::Default
            }
        );

        let package_format = PackageFormatAndCompression::from_str("conda").unwrap();
        assert_eq!(
            package_format,
            PackageFormatAndCompression {
                archive_type: CondaArchiveType::Conda,
                compression_level: CompressionLevel::Default
            }
        );

        let package_format = PackageFormatAndCompression::from_str("tar-bz2:1").unwrap();
        assert_eq!(
            package_format,
            PackageFormatAndCompression {
                archive_type: CondaArchiveType::TarBz2,
                compression_level: CompressionLevel::Numeric(1)
            }
        );

        let package_format = PackageFormatAndCompression::from_str(".tar.bz2:max").unwrap();
        assert_eq!(
            package_format,
            PackageFormatAndCompression {
                archive_type: CondaArchiveType::TarBz2,
                compression_level: CompressionLevel::Highest
            }
        );

        let package_format = PackageFormatAndCompression::from_str("tarbz2:5").unwrap();
        assert_eq!(
            package_format,
            PackageFormatAndCompression {
                archive_type: CondaArchiveType::TarBz2,
                compression_level: CompressionLevel::Numeric(5)
            }
        );

        let package_format = PackageFormatAndCompression::from_str("conda:1").unwrap();
        assert_eq!(
            package_format,
            PackageFormatAndCompression {
                archive_type: CondaArchiveType::Conda,
                compression_level: CompressionLevel::Numeric(1)
            }
        );

        let package_format = PackageFormatAndCompression::from_str("conda:max").unwrap();
        assert_eq!(
            package_format,
            PackageFormatAndCompression {
                archive_type: CondaArchiveType::Conda,
                compression_level: CompressionLevel::Highest
            }
        );

        let package_format = PackageFormatAndCompression::from_str("conda:-5").unwrap();
        assert_eq!(
            package_format,
            PackageFormatAndCompression {
                archive_type: CondaArchiveType::Conda,
                compression_level: CompressionLevel::Numeric(-5)
            }
        );

        let package_format = PackageFormatAndCompression::from_str("conda:fast").unwrap();
        assert_eq!(
            package_format,
            PackageFormatAndCompression {
                archive_type: CondaArchiveType::Conda,
                compression_level: CompressionLevel::Lowest
            }
        );
    }
}
