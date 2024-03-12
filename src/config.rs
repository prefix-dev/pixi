use clap::Parser;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{Channel, ChannelConfig, ParseChannelError};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::consts;

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
        dirs::home_dir().map(|path| path.join(".pixi"))
    }
}

/// Returns the default cache directory.
/// Most important is the `PIXI_CACHE_DIR` environment variable.
/// If that is not set, the `RATTLER_CACHE_DIR` environment variable is used.
/// If that is not set, the default cache directory of [`rattler::default_cache_dir`] is used.
pub fn get_cache_dir() -> miette::Result<PathBuf> {
    std::env::var("PIXI_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("RATTLER_CACHE_DIR").map(PathBuf::from))
        .or_else(|_| {
            rattler::default_cache_dir()
                .map_err(|_| miette::miette!("could not determine default cache directory"))
        })
}
#[derive(Parser, Debug, Default, Clone)]
pub struct ConfigCli {
    /// Do not verify the TLS certificate of the server.
    #[arg(long)]
    tls_no_verify: Option<bool>,
}

#[derive(Parser, Debug, Default, Clone)]
pub struct ConfigCliPrompt {
    #[clap(flatten)]
    config: ConfigCli,

    /// Do not change the PS1 variable when starting a prompt.
    #[arg(long)]
    change_ps1: Option<bool>,
}

#[derive(Clone, Default, Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub default_channels: Vec<String>,

    /// If set to true, pixi will set the PS1 environment variable to a custom value.
    #[serde(default)]
    change_ps1: Option<bool>,

    /// If set to true, pixi will not verify the TLS certificate of the server.
    #[serde(default)]
    tls_no_verify: Option<bool>,

    #[serde(skip)]
    pub loaded_from: Vec<PathBuf>,

    #[serde(skip)]
    pub channel_config: ChannelConfig,
}

impl From<ConfigCli> for Config {
    fn from(cli: ConfigCli) -> Self {
        Self {
            tls_no_verify: cli.tls_no_verify,
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
    pub fn from_toml(toml: &str, location: &Path) -> miette::Result<Config> {
        let mut config: Config = toml_edit::de::from_str(toml)
            .into_diagnostic()
            .context("Failed to parse config.toml")?;

        config.loaded_from.push(location.to_path_buf());

        Ok(config)
    }

    /// Load the global config file from the home directory (~/.pixi/config.toml)
    pub fn load_global() -> Config {
        let global_locations = vec![
            dirs::config_dir().map(|d| d.join(consts::PIXI_DIR).join(consts::CONFIG_FILE)),
            home_path().map(|d| d.join(consts::PIXI_DIR).join(consts::CONFIG_FILE)),
        ];
        let mut merged_config = Config::default();
        for location in global_locations {
            if let Some(location) = location {
                if location.exists() {
                    let global_config = fs::read_to_string(&location).unwrap_or_default();
                    if let Ok(config) = Config::from_toml(&global_config, &location) {
                        merged_config.merge_config(&config);
                    } else {
                        eprintln!(
                            "Could not load global config (invalid toml): {}",
                            location.display()
                        );
                    }
                }
            }
        }
        merged_config
    }

    /// Load the config from the given path pixi folder and merge it with the global config.
    pub fn from_path(p: &Path) -> miette::Result<Config> {
        let local_config = p.join(consts::CONFIG_FILE);
        let mut config = Self::load_global();

        if local_config.exists() {
            let s = fs::read_to_string(&local_config).into_diagnostic()?;
            let local = Config::from_toml(&s, &local_config);
            if let Ok(local) = local {
                config.merge_config(&local);
            }
        }

        Ok(config)
    }

    /// Merge the given config into the current one.
    pub fn merge_config(&mut self, other: &Config) {
        if !other.default_channels.is_empty() {
            self.default_channels = other.default_channels.clone();
        }

        if other.change_ps1.is_some() {
            self.change_ps1 = other.change_ps1;
        }

        if other.tls_no_verify.is_some() {
            self.tls_no_verify = other.tls_no_verify;
        }

        self.loaded_from.extend(other.loaded_from.iter().cloned());
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

    pub fn channel_config(&self) -> &ChannelConfig {
        &self.channel_config
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
}
