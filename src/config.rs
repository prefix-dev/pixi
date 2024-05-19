use clap::{ArgAction, Parser};
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{Channel, ChannelConfig, ParseChannelError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use url::Url;

use crate::consts;
use crate::util::default_channel_config;

/// Determines the default author based on the default git author. Both the name and the email
/// address of the author are returned.
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
pub fn home_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PIXI_HOME") {
        Some(PathBuf::from(path))
    } else {
        dirs::home_dir().map(|path| path.join(consts::PIXI_DIR))
    }
}

/// Returns the default cache directory.
/// Most important is the `PIXI_CACHE_DIR` environment variable.
/// - If that is not set, the `RATTLER_CACHE_DIR` environment variable is used.
/// - If that is not set, `XDG_CACHE_HOME/pixi` is used when the directory exists.
/// - If that is not set, the default cache directory of [`rattler::default_cache_dir`] is used.
pub fn get_cache_dir() -> miette::Result<PathBuf> {
    std::env::var("PIXI_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("RATTLER_CACHE_DIR").map(PathBuf::from))
        .or_else(|_| {
            let xdg_cache_pixi_dir = std::env::var_os("XDG_CACHE_HOME")
                .map_or_else(
                    || dirs::home_dir().map(|d| d.join(".cache")),
                    |p| Some(PathBuf::from(p)),
                )
                .map(|d| d.join("pixi"));

            // Only use the xdg cache pixi directory when it exists
            xdg_cache_pixi_dir
                .and_then(|d| d.exists().then_some(d))
                .ok_or_else(|| miette::miette!("could not determine xdg cache directory"))
        })
        .or_else(|_| {
            rattler::default_cache_dir()
                .map_err(|_| miette::miette!("could not determine default cache directory"))
        })
}
#[derive(Parser, Debug, Default, Clone)]
pub struct ConfigCli {
    /// Do not verify the TLS certificate of the server.
    #[arg(long, action = ArgAction::SetTrue)]
    tls_no_verify: bool,

    /// Path to the file containing the authentication token.
    #[arg(long, env = "RATTLER_AUTH_FILE")]
    auth_file: Option<PathBuf>,

    /// Specifies if we want to use uv keyring provider
    #[arg(long)]
    pypi_keyring_provider: Option<KeyringProvider>,
}

#[derive(Parser, Debug, Default, Clone)]
pub struct ConfigCliPrompt {
    #[clap(flatten)]
    config: ConfigCli,

    /// Do not change the PS1 variable when starting a prompt.
    #[arg(long)]
    change_ps1: Option<bool>,
}

#[derive(Clone, Default, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct RepodataConfig {
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
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum KeyringProvider {
    Disabled,
    Subprocess,
}

#[derive(Clone, Debug, Deserialize, Default, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Config {
    #[serde(default)]
    #[serde(alias = "default_channels")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub default_channels: Vec<String>,

    /// If set to true, pixi will set the PS1 environment variable to a custom value.
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

    #[serde(skip)]
    #[serde(alias = "loaded_from")] // BREAK: remove to stop supporting snake_case alias
    pub loaded_from: Vec<PathBuf>,

    #[serde(skip, default = "default_channel_config")]
    #[serde(alias = "channel_config")] // BREAK: remove to stop supporting snake_case alias
    pub channel_config: ChannelConfig,

    /// Configuration for repodata fetching.
    #[serde(alias = "repodata_config")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repodata_config: Option<RepodataConfig>,

    /// Configuration for PyPI packages.
    #[serde(default)]
    #[serde(skip_serializing_if = "PyPIConfig::is_default")]
    pub pypi_config: PyPIConfig,
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
            repodata_config: None,
            pypi_config: PyPIConfig::default(),
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
            ..Default::default()
        }
    }
}

impl From<ConfigCliPrompt> for Config {
    fn from(cli: ConfigCliPrompt) -> Self {
        let mut config: Config = cli.config.into();
        config.change_ps1 = cli.change_ps1;
        config
    }
}

impl Config {
    /// Parse the given toml string and return a Config instance.
    ///
    /// # Returns
    ///
    /// The parsed config
    ///
    /// # Errors
    ///
    /// Parsing errors
    #[inline]
    pub fn from_toml(toml: &str) -> miette::Result<Config> {
        toml_edit::de::from_str(toml).into_diagnostic()
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
    pub fn from_path(path: &Path) -> miette::Result<Config> {
        tracing::debug!("Loading config from {}", path.display());
        let s = fs::read_to_string(path)
            .into_diagnostic()
            .wrap_err(format!("failed to read config from '{}'", path.display()))?;
        let mut config = Config::from_toml(&s)?;
        config.loaded_from.push(path.to_path_buf());
        tracing::info!("Loaded config from: {}", path.display());

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
    pub fn try_load_system() -> miette::Result<Config> {
        Self::from_path(&config_path_system())
    }

    /// Load the system config file from the system path.
    ///
    /// # Returns
    ///
    /// The loaded system config
    pub fn load_system() -> Config {
        Self::try_load_system().unwrap_or_else(|e| {
            let path = config_path_system();
            tracing::debug!(
                "Failed to load system config: {} (error: {})",
                path.display(),
                e
            );
            Self::default()
        })
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
                Err(e) => tracing::debug!(
                    "Failed to load global config: {} (error: {})",
                    p.display(),
                    e
                ),
            }
        }

        // Load the default CLI config and layer it on top of the global config
        // This will add any environment variables defined in the `clap` attributes to the config
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

    /// Merge the given config into the current one.
    #[must_use]
    pub fn merge_config(mut self, other: Config) -> Self {
        self.mirrors.extend(other.mirrors);
        self.loaded_from.extend(other.loaded_from);

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
            mirrors: self.mirrors,
            loaded_from: self.loaded_from,
            // currently this is always the default so just use the other value
            channel_config: other.channel_config,
            repodata_config: other.repodata_config.or(self.repodata_config),
            pypi_config: other.pypi_config.merge(self.pypi_config),
        }
    }

    /// Retrieve the value for the default_channels field (defaults to the ["conda-forge"]).
    pub fn default_channels(&self) -> Vec<String> {
        if self.default_channels.is_empty() {
            consts::DEFAULT_CHANNELS
                .iter()
                .map(|s| s.to_string())
                .collect()
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

    pub fn channel_config(&self) -> &ChannelConfig {
        &self.channel_config
    }

    pub fn repodata_config(&self) -> Option<&RepodataConfig> {
        self.repodata_config.as_ref()
    }

    pub fn pypi_config(&self) -> &PyPIConfig {
        &self.pypi_config
    }

    pub fn compute_channels(
        &self,
        cli_channels: &[String],
    ) -> Result<Vec<Channel>, ParseChannelError> {
        let channels = if cli_channels.is_empty() {
            self.default_channels()
        } else {
            cli_channels.to_vec()
        };

        channels
            .iter()
            .map(|c| Channel::from_str(c, &self.channel_config))
            .collect::<Result<Vec<Channel>, _>>()
    }

    pub fn mirror_map(&self) -> &std::collections::HashMap<Url, Vec<Url>> {
        &self.mirrors
    }

    /// Modify this config with the given key and value
    ///
    /// # Note
    ///
    /// It is required to call `save()` to persist the changes.
    pub fn set(&mut self, key: &str, value: Option<String>) -> miette::Result<()> {
        let show_supported_keys = || {
            let keys = [
                "default-channels",
                "change-ps1",
                "authentication-override-file",
                "tls-no-verify",
                "mirrors",
                "repodata-config",
                "repodata-config.disable-jlap",
                "repodata-config.disable-bzip2",
                "repodata-config.disable-zstd",
                "pypi-config",
                "pypi-config.index-url",
                "pypi-config.extra-index-urls",
                "pypi-config.keyring-provider",
            ];
            format!("Supported keys:\n\n{}", keys.join("\n"))
        };

        let err = miette::miette!("Unknown key: {}\n{}", key, show_supported_keys());

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
            key if key.starts_with("repodata-config") => {
                if key == "repodata-config" {
                    self.repodata_config = value
                        .map(|v| serde_json::de::from_str(&v))
                        .transpose()
                        .into_diagnostic()?;
                    return Ok(());
                } else if !key.starts_with("repodata-config.") {
                    return Err(err);
                }

                let subkey = key.strip_prefix("repodata-config.").unwrap();
                match subkey {
                    "disable-jlap" => {
                        self.repodata_config
                            .get_or_insert(RepodataConfig::default())
                            .disable_jlap =
                            value.map(|v| v.parse()).transpose().into_diagnostic()?;
                    }
                    "disable-bzip2" => {
                        self.repodata_config
                            .get_or_insert(RepodataConfig::default())
                            .disable_bzip2 =
                            value.map(|v| v.parse()).transpose().into_diagnostic()?;
                    }
                    "disable-zstd" => {
                        self.repodata_config
                            .get_or_insert(RepodataConfig::default())
                            .disable_zstd =
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
            _ => return Err(err),
        }

        Ok(())
    }

    /// Save the config to the given path.
    pub fn save(&self, to: &Path) -> miette::Result<()> {
        let contents = toml_edit::ser::to_string_pretty(&self).into_diagnostic()?;
        tracing::debug!("Saving config to: {}", to.display());

        let parent = to.parent().expect("config path should have a parent");
        fs::create_dir_all(parent)
            .into_diagnostic()
            .wrap_err(format!(
                "failed to create directories in '{}'",
                parent.display()
            ))?;
        fs::write(to, contents)
            .into_diagnostic()
            .wrap_err(format!("failed to write config to '{}'", to.display()))
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

    base_path.join("pixi").join(consts::CONFIG_FILE)
}

/// Returns the path(s) to the global pixi config file.
pub fn config_path_global() -> Vec<PathBuf> {
    let xdg_config_home = std::env::var_os("XDG_CONFIG_HOME").map_or_else(
        || dirs::home_dir().map(|d| d.join(".config")),
        |p| Some(PathBuf::from(p)),
    );

    vec![
        xdg_config_home.map(|d| d.join("pixi").join(consts::CONFIG_FILE)),
        dirs::config_dir().map(|d| d.join("pixi").join(consts::CONFIG_FILE)),
        home_path().map(|d| d.join(consts::CONFIG_FILE)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_parse() {
        let toml = r#"
        default_channels = ["conda-forge"]
        tls_no_verify = true
        "#;
        let config = Config::from_toml(toml).unwrap();
        assert_eq!(config.default_channels, vec!["conda-forge"]);
        assert_eq!(config.tls_no_verify, Some(true));
    }

    #[test]
    fn test_config_from_cli() {
        let cli = ConfigCli {
            tls_no_verify: true,
            auth_file: None,
            pypi_keyring_provider: Some(KeyringProvider::Subprocess),
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
        };

        let config = Config::from(cli);
        assert_eq!(config.tls_no_verify, None);
        assert_eq!(
            config.authentication_override_file,
            Some(PathBuf::from("path.json"))
        );
    }

    #[test]
    fn test_pypi_config_parse() {
        let toml = r#"
            [pypi-config]
            index-url = "https://pypi.org/simple"
            extra-index-urls = ["https://pypi.org/simple2"]
            keyring-provider = "subprocess"
        "#;
        let config = Config::from_toml(toml).unwrap();
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
    fn test_config_merge() {
        let mut config = Config::default();
        let other = Config {
            default_channels: vec!["conda-forge".to_string()],
            channel_config: ChannelConfig::default_with_root_dir(PathBuf::from("/root/dir")),
            tls_no_verify: Some(true),
            ..Default::default()
        };
        config = config.merge_config(other);
        assert_eq!(config.default_channels, vec!["conda-forge"]);
        assert_eq!(config.tls_no_verify, Some(true));

        let d = Path::new(&env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("config");

        let config_1 = Config::from_path(&d.join("config_1.toml")).unwrap();
        let config_2 = Config::from_path(&d.join("config_2.toml")).unwrap();
        let config_2 = Config {
            channel_config: ChannelConfig::default_with_root_dir(PathBuf::from("/root/dir")),
            ..config_2
        };

        let mut merged = config_1.clone();
        merged = merged.merge_config(config_2);

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
        let config = Config::from_toml(toml).unwrap();
        assert_eq!(config.default_channels, vec!["conda-forge"]);
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
        let repodata_config = config.repodata_config.unwrap();
        assert_eq!(repodata_config.disable_jlap, Some(true));
        assert_eq!(repodata_config.disable_bzip2, Some(true));
        assert_eq!(repodata_config.disable_zstd, Some(true));
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
        "#;
        Config::from_toml(toml).unwrap();
    }

    #[test]
    fn test_alter_config() {
        let mut config = Config::default();
        config
            .set("default-channels", Some(r#"["conda-forge"]"#.to_string()))
            .unwrap();
        assert_eq!(config.default_channels, vec!["conda-forge"]);

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
        let repodata_config = config.repodata_config().unwrap();
        assert_eq!(repodata_config.disable_jlap, Some(true));

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

        config.set("unknown-key", None).unwrap_err();
    }
}
