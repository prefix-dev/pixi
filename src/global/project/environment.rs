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
    .collect::<Vec<_>>();

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
    let mut to_add = IndexSet::new();
    for (exposed_name, _) in exposed.iter() {
        if related_exposed
            .iter()
            .map(|path| executable_from_path(path))
            .any(|exec| exec == exposed_name.to_string())
        {
            to_add.insert(exposed_name.clone());
        }
    }

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
    use fs_err as fs;
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
        fs::create_dir_all(&target_dir).unwrap();
        let test_data_target = target_dir.join(file_name);
        let test_data_source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/global/test_data/conda-meta")
            .join(file_name);
        fs::copy(test_data_source, test_data_target).unwrap();

        let result = environment_specs_in_sync(&env_dir, &specs, None)
            .await
            .unwrap();
        assert!(result);
    }
}
