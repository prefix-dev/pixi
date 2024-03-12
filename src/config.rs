use miette::{Context, IntoDiagnostic};
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

fn change_ps1_default() -> bool {
    true
}

#[derive(Clone, Default, Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub default_channels: Vec<String>,

    /// If set to true, pixi will set the PS1 environment variable to a custom value.
    #[serde(default = "change_ps1_default")]
    pub change_ps1: bool,

    /// If set to true, pixi will not verify the TLS certificate of the server.
    #[serde(default)]
    pub tls_no_verify: bool,
}

impl Config {
    pub fn from_toml(toml: &str) -> miette::Result<Config> {
        toml_edit::de::from_str(toml)
            .into_diagnostic()
            .context("Failed to parse config.toml")
    }

    /// Load the global config file from the home directory (~/.pixi/config.toml)
    pub fn load_global() -> Config {
        let global_config = dirs::home_dir()
            .map(|d| d.join(consts::PIXI_DIR).join(consts::CONFIG_FILE))
            .and_then(|p| fs::read_to_string(p).ok());

        if let Some(global_config) = global_config {
            if let Ok(config) = Config::from_toml(&global_config) {
                return config;
            }
            eprintln!(
                "Could not load global config (invalid toml): ~/{}/{}",
                consts::PIXI_DIR,
                consts::CONFIG_FILE
            );
        }

        Config::default()
    }

    /// Load the config from the given path pixi folder and merge it with the global config.
    pub fn from_path(p: &Path) -> miette::Result<Config> {
        let local_config = p.join(consts::CONFIG_FILE);
        let mut config = Self::load_global();

        if local_config.exists() {
            let s = fs::read_to_string(&local_config).into_diagnostic()?;
            let local = Config::from_toml(&s);
            if let Ok(local) = local {
                config.merge_config(&local);
            }
        }

        Ok(config)
    }

    pub fn merge_config(&mut self, other: &Config) {
        if !other.default_channels.is_empty() {
            self.default_channels = other.default_channels.clone();
        }
    }
}
