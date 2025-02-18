use clap::{ArgAction, Parser};
use itertools::Itertools;
use miette::{miette, Context, IntoDiagnostic};
use pixi_consts::consts;
use rattler_conda_types::{
    version_spec::{EqualityOperator, LogicalOperator, RangeOperator},
    ChannelConfig, NamedChannelOrUrl, Version, VersionBumpType, VersionSpec,
};
use rattler_networking::s3_middleware;
use rattler_repodata_gateway::{Gateway, SourceConfig};
use reqwest_middleware::ClientWithMiddleware;
use serde::{de::IntoDeserializer, Deserialize, Serialize};
use std::{
    collections::{BTreeSet as Set, HashMap},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str::FromStr,
};
use url::Url;

const EXPERIMENTAL: &str = "experimental";

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
    /// Do not verify the TLS certificate of the server.
    #[arg(long, action = ArgAction::SetTrue)]
    tls_no_verify: bool,

    /// Path to the file containing the authentication token.
    #[arg(long)]
    auth_file: Option<PathBuf>,

    /// Specifies if we want to use uv keyring provider
    #[arg(long)]
    pypi_keyring_provider: Option<KeyringProvider>,

    /// Max concurrent solves, default is the number of CPUs
    #[arg(long)]
    pub concurrent_solves: Option<usize>,

    /// Max concurrent network requests, default is 50
    #[arg(long)]
    pub concurrent_downloads: Option<usize>,
}

#[derive(Parser, Debug, Clone, Default)]
pub struct ConfigCliPrompt {
    /// Do not change the PS1 variable when starting a prompt.
    #[arg(long)]
    change_ps1: Option<bool>,
}
impl From<ConfigCliPrompt> for Config {
    fn from(cli: ConfigCliPrompt) -> Self {
        Self {
            change_ps1: cli.change_ps1,
            ..Default::default()
        }
    }
}

impl ConfigCliPrompt {
    pub fn merge_config(self, config: Config) -> Config {
        let mut config = config;
        config.change_ps1 = self.change_ps1.or(config.change_ps1);
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
    /// Do not use the environment activation cache. (default: true except in experimental mode)
    #[arg(long)]
    force_activate: bool,
}

impl ConfigCliActivation {
    pub fn merge_config(self, config: Config) -> Config {
        let mut config = config;
        config.force_activate = Some(self.force_activate);
        config
    }
}

impl From<ConfigCliActivation> for Config {
    fn from(cli: ConfigCliActivation) -> Self {
        Self {
            force_activate: Some(cli.force_activate),
            ..Default::default()
        }
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
            jlap_enabled: !value.disable_jlap.unwrap_or(false),
            zstd_enabled: !value.disable_zstd.unwrap_or(false),
            bz2_enabled: !value.disable_bzip2.unwrap_or(false),
            // TODO: Change sharded repodata default to true, when enough testing has been done.
            sharded_enabled: !value.disable_sharded.unwrap_or(true),
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
        match self {
            DetachedEnvironments::Path(p) => Ok(Some(p.clone())),
            DetachedEnvironments::Boolean(b) if *b => {
                let path = get_cache_dir()?.join(consts::ENVIRONMENTS_DIR);
                Ok(Some(path))
            }
            _ => Ok(None),
        }
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
    /// This is an experimental feature and may be removed in the future or made default.
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

// Making the default values part of pixi_config to allow for printing the default settings in the future.
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
    // Needing to set this default next to the default of the full struct to avoid serde defaulting to 0 of partial struct was omitted.
    #[serde(default = "default_max_concurrent_solves")]
    pub solves: usize,

    /// The maximum number of concurrent HTTP requests to make.
    // Needing to set this default next to the default of the full struct to avoid serde defaulting to 0 of partial struct was omitted.
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
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, Copy)]
#[serde(rename_all = "kebab-case")]
pub enum PinningStrategy {
    /// Default semver strategy e.g. "1.2.3" becomes ">=1.2.3, <2" but "0.1.0"
    /// becomes ">=0.1.0, <0.2"
    #[default]
    Semver,
    /// Pin the latest minor e.g. "1.2.3" becomes ">=1.2.3, <1.3"
    Minor,
    /// Pin the latest major e.g. "1.2.3" becomes ">=1.2.3, <2"
    Major,
    /// Pin to the latest version or higher. e.g. "1.2.3" becomes ">=1.2.3"
    LatestUp,
    /// Pin the version chosen by the solver. e.g. "1.2.3" becomes "==1.2.3"
    // Adding "Version" to the name for future extendability.
    ExactVersion,
    /// No pinning, keep the requirement empty. e.g. "1.2.3" becomes "*"
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct Config {
    #[serde(default)]
    #[serde(alias = "default_channels")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub default_channels: Vec<NamedChannelOrUrl>,

    /// If set to true, pixi will set the PS1 environment variable to a custom
    /// value.
    #[serde(default)]
    #[serde(alias = "change_ps1")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_ps1: Option<bool>,

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
    channel_config: ChannelConfig,

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

    /// The option to disable the environment activation cache
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force_activate: Option<bool>,

    /// Experimental features that can be enabled.
    #[serde(default)]
    #[serde(skip_serializing_if = "ExperimentalConfig::is_default")]
    pub experimental: ExperimentalConfig,

    /// Concurrency configuration for pixi
    #[serde(default)]
    #[serde(skip_serializing_if = "ConcurrencyConfig::is_default")]
    pub concurrency: ConcurrencyConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_channels: Vec::new(),
            change_ps1: None,
            authentication_override_file: None,
            tls_no_verify: None,
            mirrors: HashMap::new(),
            loaded_from: Vec::new(),
            channel_config: default_channel_config(),
            repodata_config: RepodataConfig::default(),
            pypi_config: PyPIConfig::default(),
            s3_options: HashMap::new(),
            detached_environments: None,
            pinning_strategy: None,
            force_activate: None,
            experimental: ExperimentalConfig::default(),
            concurrency: ConcurrencyConfig::default(),
        }
    }
}

impl From<ConfigCli> for Config {
    fn from(cli: ConfigCli) -> Self {
        Self {
            tls_no_verify: if cli.tls_no_verify { Some(true) } else { None },
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
    pub fn from_toml(toml: &str) -> miette::Result<(Config, Set<String>)> {
        let de = toml_edit::de::Deserializer::from_str(toml).into_diagnostic()?;

        // Deserialize the config and collect unused keys
        let mut unused_keys = Set::new();
        let config: Config = serde_ignored::deserialize(de, |path| {
            unused_keys.insert(path.to_string());
        })
        .into_diagnostic()?;

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
                return Err(ConfigError::FileNotFound(path.to_path_buf()))
            }
            Err(e) => return Err(ConfigError::ReadError(e)),
        };

        let (mut config, unused_keys) =
            Config::from_toml(&s).map_err(|e| ConfigError::ParseError(e, path.to_path_buf()))?;

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
        tracing::info!("Loaded config from: {}", path.display());

        config
            .validate()
            .map_err(|e| ConfigError::ValidationError(e, path.to_path_buf()))?;

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
        if let Some(detached_environments) = self.detached_environments.clone() {
            match detached_environments {
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
            "default-channels",
            "change-ps1",
            "authentication-override-file",
            "tls-no-verify",
            "mirrors",
            "detached-environments",
            "pinning-strategy",
            "max-concurrent-solves",
            "repodata-config",
            "repodata-config.disable-jlap",
            "repodata-config.disable-bzip2",
            "repodata-config.disable-zstd",
            "repodata-config.disable-sharded",
            "pypi-config",
            "pypi-config.index-url",
            "pypi-config.extra-index-urls",
            "pypi-config.keyring-provider",
            "s3-options",
            "s3-options.<bucket>",
            "s3-options.<bucket>.endpoint-url",
            "s3-options.<bucket>.region",
            "s3-options.<bucket>.force-path-style",
            "experimental.use-environment-activation-cache",
        ]
    }

    /// Merge the given config into the current one.
    /// The given config will have higher priority
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
            change_ps1: other.change_ps1.or(self.change_ps1),
            authentication_override_file: other
                .authentication_override_file
                .or(self.authentication_override_file),
            // Extended self.mirrors with other.mirrors
            mirrors: self.mirrors,
            loaded_from: other.loaded_from,
            // currently this is always the default so just use the other value
            channel_config: other.channel_config,
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
            force_activate: other.force_activate,
            experimental: self.experimental.merge(other.experimental),
            // Make other take precedence over self to allow for setting the value through the CLI
            concurrency: self.concurrency.merge(other.concurrency),
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

    /// Retrieve the value for the change_ps1 field (defaults to true).
    pub fn change_ps1(&self) -> bool {
        self.change_ps1.unwrap_or(true)
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
        self.force_activate.unwrap_or(false)
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
            "change-ps1" => {
                self.change_ps1 = value.map(|v| v.parse()).transpose().into_diagnostic()?;
            }
            "authentication-override-file" => {
                self.authentication_override_file = value.map(PathBuf::from);
            }
            "tls-no-verify" => {
                self.tls_no_verify = value.map(|v| v.parse()).transpose().into_diagnostic()?;
            }
            "mirrors" => {
                self.mirrors = value
                    .map(|v| serde_json::de::from_str(&v))
                    .transpose()
                    .into_diagnostic()?
                    .unwrap_or_default();
            }
            "detached-environments" => {
                self.detached_environments = value.map(|v| match v.as_str() {
                    "true" => DetachedEnvironments::Boolean(true),
                    "false" => DetachedEnvironments::Boolean(false),
                    _ => DetachedEnvironments::Path(PathBuf::from(v)),
                });
            }
            "pinning-strategy" => {
                self.pinning_strategy = value
                    .map(|v| PinningStrategy::from_str(v.as_str()))
                    .transpose()
                    .into_diagnostic()?
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

    /// Constructs a [`Gateway`] using a [`ClientWithMiddleware`]
    pub fn gateway(&self, client: ClientWithMiddleware) -> Gateway {
        // Determine the cache directory and fall back to sane defaults otherwise.
        let cache_dir = get_cache_dir().unwrap_or_else(|e| {
            tracing::error!("failed to determine repodata cache directory: {e}");
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("./"))
        });

        // Construct the gateway
        Gateway::builder()
            .with_client(client)
            .with_cache_dir(cache_dir.join(consts::CONDA_REPODATA_CACHE_DIR))
            .with_channel_config(self.into())
            .with_max_concurrent_requests(self.max_concurrent_downloads())
            .finish()
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
detached-environments = "{}"
pinning-strategy = "no-pin"
concurrency.solves = 5
UNUSED = "unused"
        "#,
            env!("CARGO_MANIFEST_DIR").replace('\\', "\\\\").as_str()
        );
        let (config, unused) = Config::from_toml(toml.as_str()).unwrap();
        assert_eq!(
            config.default_channels,
            vec![NamedChannelOrUrl::from_str("conda-forge").unwrap()]
        );
        assert_eq!(config.tls_no_verify, Some(true));
        assert_eq!(
            config.detached_environments().path().unwrap(),
            Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")))
        );
        assert_eq!(config.max_concurrent_solves(), 5);
        assert!(unused.contains("UNUSED"));

        let toml = r"detached-environments = true";
        let (config, _) = Config::from_toml(toml).unwrap();
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
        let toml = format!("pinning-strategy = \"{}\"", input);
        let (config, _) = Config::from_toml(&toml).unwrap();
        assert_eq!(config.pinning_strategy, Some(expected));
    }

    #[test]
    fn test_config_from_cli() {
        let cli = ConfigCli {
            tls_no_verify: true,
            auth_file: None,
            pypi_keyring_provider: Some(KeyringProvider::Subprocess),
            concurrent_solves: None,
            concurrent_downloads: None,
        };
        let config = Config::from(cli);
        assert_eq!(config.tls_no_verify, Some(true));
        assert_eq!(
            config.pypi_config().keyring_provider,
            Some(KeyringProvider::Subprocess)
        );

        let cli = ConfigCli {
            tls_no_verify: false,
            auth_file: Some(PathBuf::from("path.json")),
            pypi_keyring_provider: None,
            concurrent_solves: None,
            concurrent_downloads: None,
        };

        let config = Config::from(cli);
        assert_eq!(config.tls_no_verify, None);
        assert_eq!(
            config.authentication_override_file,
            Some(PathBuf::from("path.json"))
        );
        assert!(!config.experimental.use_environment_activation_cache());
    }

    #[test]
    fn test_pypi_config_parse() {
        let toml = r#"
            [pypi-config]
            index-url = "https://pypi.org/simple"
            extra-index-urls = ["https://pypi.org/simple2"]
            keyring-provider = "subprocess"
        "#;
        let (config, _) = Config::from_toml(toml).unwrap();
        assert_eq!(
            config.pypi_config().index_url,
            Some(Url::parse("https://pypi.org/simple").unwrap())
        );
        assert!(config.pypi_config().extra_index_urls.len() == 1);
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
        let (config, _) = Config::from_toml(toml).unwrap();
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
        let (config, _) = Config::from_toml(toml).unwrap();
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
        let result = Config::from_toml(toml);
        assert!(result.is_err());
        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("missing field `force-path-style`"));
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
            detached_environments: Some(DetachedEnvironments::Path(PathBuf::from("/path/to/envs"))),
            concurrency: ConcurrencyConfig {
                solves: 5,
                ..ConcurrencyConfig::default()
            },
            change_ps1: Some(false),
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
            force_activate: Some(true),
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
        assert!(config.s3_options["bucket2"]
            .endpoint_url
            .to_string()
            .contains("my-new-s3-host"));

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

        let debug = format!("{:#?}", merged);
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
        let (config, _) = Config::from_toml(toml).unwrap();
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
        Config::from_toml(toml).unwrap();
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

        config.set("change-ps1", None).unwrap();
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
        assert!(s3_options
            .endpoint_url
            .to_string()
            .contains("http://localhost:9000"));
        assert!(s3_options.force_path_style);
        assert_eq!(s3_options.region, "auto");

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
        let (config, _) = Config::from_toml(toml).unwrap();
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
}
