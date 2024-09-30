use crate::global::install::local_environment_matches_spec;
use crate::global::{extract_executable_from_script, BinDir, EnvDir, ExposedName};
use crate::prefix::Prefix;
use console::StyledObject;
use fancy_display::FancyDisplay;
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use miette::Diagnostic;
use pixi_consts::consts;
use pixi_utils::executable_from_path;
use rattler_conda_types::{MatchSpec, Platform};
use regex::Regex;
use serde::{self, Deserialize, Deserializer, Serialize};
use std::path::PathBuf;
use std::{fmt, str::FromStr};
use thiserror::Error;

/// Represents the name of an environment.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize)]
pub(crate) struct EnvironmentName(String);

impl EnvironmentName {
    /// Returns the name of the environment.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EnvironmentName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl PartialEq<str> for EnvironmentName {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl<'de> Deserialize<'de> for EnvironmentName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let name = String::deserialize(deserializer)?;
        name.parse().map_err(serde::de::Error::custom)
    }
}

impl FancyDisplay for &EnvironmentName {
    fn fancy_display(&self) -> StyledObject<&str> {
        consts::ENVIRONMENT_STYLE.apply_to(self.as_str())
    }
}

impl FromStr for EnvironmentName {
    type Err = ParseEnvironmentNameError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        static REGEX: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        let regex = REGEX
            .get_or_init(|| Regex::new(r"^[a-z0-9-]+$").expect("Regex should be able to compile"));

        if !regex.is_match(s) {
            // Return an error if the string does not match the regex
            return Err(ParseEnvironmentNameError {
                attempted_parse: s.to_string(),
            });
        }
        Ok(EnvironmentName(s.to_string()))
    }
}

/// Represents an error that occurs when parsing an environment name.
///
/// This error is returned when a string fails to be parsed as an environment name.
#[derive(Debug, Clone, Error, Diagnostic, PartialEq)]
#[error("Failed to parse environment name '{attempted_parse}', please use only lowercase letters, numbers and dashes")]
pub struct ParseEnvironmentNameError {
    /// The string that was attempted to be parsed.
    pub attempted_parse: String,
}

/// Figures out what the status is of the exposed binaries of the environment.
///
/// Returns a tuple of the exposed binaries to remove and the exposed binaries to add.
pub(crate) async fn get_expose_scripts_sync_status(
    bin_dir: &BinDir,
    env_dir: &EnvDir,
    exposed: &IndexMap<ExposedName, String>,
) -> miette::Result<(IndexSet<PathBuf>, IndexSet<ExposedName>)> {
    // Get all paths to the binaries from the scripts in the bin directory.
    let locally_exposed = bin_dir.files().await?;
    let executable_paths = futures::future::join_all(locally_exposed.iter().map(|path| {
        let path = path.clone();
        async move {
            extract_executable_from_script(&path)
                .await
                .ok()
                .map(|exec| (path, exec))
        }
    }))
    .await
    .into_iter()
    .flatten()
    .collect_vec();

    // Filter out all binaries that are related to the environment
    let related_exposed = executable_paths
        .into_iter()
        .filter(|(_, exec)| exec.starts_with(env_dir.path()))
        .map(|(path, _)| path)
        .collect_vec();

    // Get all related expose scripts not required by the environment manifest
    let to_remove = related_exposed
        .iter()
        .filter(|path| {
            !exposed
                .iter()
                .any(|(exposed_name, _)| executable_from_path(path) == exposed_name.to_string())
        })
        .cloned()
        .collect::<IndexSet<PathBuf>>();

    // Get all required exposed binaries that are not yet exposed
    let to_add = exposed
        .iter()
        .filter_map(|(exposed_name, _)| {
            if related_exposed
                .iter()
                .map(|path| executable_from_path(path))
                .any(|exec| exec == exposed_name.to_string())
            {
                None
            } else {
                Some(exposed_name.clone())
            }
        })
        .collect::<IndexSet<ExposedName>>();

    Ok((to_remove, to_add))
}

/// Checks if the manifest is in sync with the locally installed environment and binaries.
/// Returns `true` if the environment is in sync, `false` otherwise.
pub(crate) async fn environment_specs_in_sync(
    env_dir: &EnvDir,
    specs: &IndexSet<MatchSpec>,
    platform: Option<Platform>,
) -> miette::Result<bool> {
    let prefix = Prefix::new(env_dir.path());

    let repodata_records = prefix
        .find_installed_packages(Some(50))
        .await?
        .into_iter()
        .map(|r| r.repodata_record)
        .collect_vec();

    if !local_environment_matches_spec(repodata_records, specs, platform) {
        return Ok(false);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::global::EnvRoot;
    use fs_err::tokio as tokio_fs;
    use rattler_conda_types::ParseStrictness;

    #[tokio::test]
    async fn test_environment_specs_in_sync() {
        let home = tempfile::tempdir().unwrap();
        let env_root = EnvRoot::new(home.into_path()).unwrap();
        let env_name = EnvironmentName::from_str("test").unwrap();
        let env_dir = EnvDir::from_env_root(env_root, env_name).await.unwrap();

        // Test empty
        let specs = IndexSet::new();
        let result = environment_specs_in_sync(&env_dir, &specs, None)
            .await
            .unwrap();
        assert!(result);

        // Test with spec
        let mut specs = IndexSet::new();
        specs.insert(MatchSpec::from_str("_r-mutex==1.0.1", ParseStrictness::Strict).unwrap());
        // Copy from test data folder relative to this file to the conda-meta in environment directory
        let file_name = "_r-mutex-1.0.1-anacondar_1.json";
        let target_dir = PathBuf::from(env_dir.path()).join("conda-meta");
        tokio_fs::create_dir_all(&target_dir).await.unwrap();
        let test_data_target = target_dir.join(file_name);
        let test_data_source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/global/test_data/conda-meta")
            .join(file_name);
        tokio_fs::copy(test_data_source, test_data_target)
            .await
            .unwrap();

        let result = environment_specs_in_sync(&env_dir, &specs, None)
            .await
            .unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn test_get_expose_scripts_sync_status() {
        let tmp_home_dir = tempfile::tempdir().unwrap();
        let tmp_home_dir_path = tmp_home_dir.path().to_path_buf();
        let env_root = EnvRoot::new(tmp_home_dir_path.clone()).unwrap();
        let env_name = EnvironmentName::from_str("test").unwrap();
        let env_dir = EnvDir::from_env_root(env_root, env_name).await.unwrap();
        let bin_dir = BinDir::new(tmp_home_dir_path.clone()).unwrap();

        // Test empty
        let exposed = IndexMap::new();
        let (to_remove, to_add) = get_expose_scripts_sync_status(&bin_dir, &env_dir, &exposed)
            .await
            .unwrap();
        assert!(to_remove.is_empty());
        assert!(to_add.is_empty());

        // Test with exposed
        let mut exposed = IndexMap::new();
        exposed.insert(ExposedName::from_str("test").unwrap(), "test".to_string());
        let (to_remove, to_add) = get_expose_scripts_sync_status(&bin_dir, &env_dir, &exposed)
            .await
            .unwrap();
        assert!(to_remove.is_empty());
        assert_eq!(to_add.len(), 1);

        // Add a script to the bin directory
        let script_path = if cfg!(windows) {
            bin_dir.path().join("test.bat")
        } else {
            bin_dir.path().join("test")
        };

        let script = if cfg!(windows) {
            format!(
                r#"
            @"{}" %*
            "#,
                env_dir
                    .path()
                    .join("bin")
                    .join("test.exe")
                    .to_string_lossy()
            )
        } else {
            format!(
                r#"#!/bin/sh
            "{}" "$@"
            "#,
                env_dir.path().join("bin").join("test").to_string_lossy()
            )
        };
        tokio_fs::write(script_path, script).await.unwrap();

        let (to_remove, to_add) = get_expose_scripts_sync_status(&bin_dir, &env_dir, &exposed)
            .await
            .unwrap();
        assert!(to_remove.is_empty());
        assert!(to_add.is_empty());

        // Test to_remove
        let (to_remove, to_add) =
            get_expose_scripts_sync_status(&bin_dir, &env_dir, &IndexMap::new())
                .await
                .unwrap();
        assert_eq!(to_remove.len(), 1);
        assert!(to_add.is_empty());
    }
}
