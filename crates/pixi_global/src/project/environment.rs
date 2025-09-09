use crate::install::local_environment_matches_spec;
use console::StyledObject;
use fancy_display::FancyDisplay;
use indexmap::IndexSet;
use miette::Diagnostic;
use pixi_consts::consts;
use rattler_conda_types::{MatchSpec, PackageName, Platform, PrefixRecord};
use regex::Regex;
use serde::{self, Deserialize, Deserializer, Serialize};
use std::collections::HashSet;
use std::{fmt, str::FromStr};
use thiserror::Error;

/// Represents the name of an environment.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize)]
pub struct EnvironmentName(String);

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

impl FancyDisplay for EnvironmentName {
    fn fancy_display(&self) -> StyledObject<&str> {
        consts::ENVIRONMENT_STYLE.apply_to(self.as_str())
    }
}

impl FromStr for EnvironmentName {
    type Err = ParseEnvironmentNameError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        static REGEX: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        let regex = REGEX.get_or_init(|| {
            Regex::new(r"^[a-z0-9-_.]+$").expect("Regex should be able to compile")
        });

        if !regex.is_match(s) {
            // Return an error if the string doesn't match the regex
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
#[error(
    "Failed to parse environment name '{attempted_parse}', please use only lowercase letters, numbers, dashes, underscores and dots"
)]
pub struct ParseEnvironmentNameError {
    /// The string that was attempted to be parsed.
    pub attempted_parse: String,
}

/// Checks if the manifest is in sync with the locally installed environment and binaries.
/// Returns `true` if the environment is in sync, `false` otherwise.
pub(crate) async fn environment_specs_in_sync(
    prefix_records: &[PrefixRecord],
    specs: &IndexSet<MatchSpec>,
    source_package_names: &HashSet<PackageName>,
    platform: Option<Platform>,
) -> miette::Result<bool> {
    let package_records = prefix_records
        .iter()
        .map(|r| r.repodata_record.package_record.clone())
        .collect();

    if !local_environment_matches_spec(package_records, specs, source_package_names, platform) {
        return Ok(false);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::{EnvDir, EnvRoot};
    use fs_err::tokio as tokio_fs;
    use pixi_utils::prefix::Prefix;
    use rattler_conda_types::ParseStrictness;
    use std::path::PathBuf;

    #[test]
    fn test_environment_name_parsing() {
        // Test that environment names allow basic characters
        assert!(EnvironmentName::from_str("test").is_ok());
        assert!(EnvironmentName::from_str("test-name").is_ok());
        assert!(EnvironmentName::from_str("test_name").is_ok());
        assert!(EnvironmentName::from_str("test123").is_ok());

        // Test that environment names with dots should work (for package names)
        assert!(EnvironmentName::from_str("my.package").is_ok());
        assert!(EnvironmentName::from_str("package.with.dots").is_ok());
        assert!(EnvironmentName::from_str("test-123.version").is_ok());

        // Test invalid characters are still rejected
        assert!(EnvironmentName::from_str("test/name").is_err());
        assert!(EnvironmentName::from_str("test name").is_err());
        assert!(EnvironmentName::from_str("Test").is_err()); // uppercase
        assert!(EnvironmentName::from_str("test:name").is_err());
    }

    #[tokio::test]
    async fn test_environment_specs_in_sync() {
        let home = tempfile::tempdir().unwrap();
        let env_root = EnvRoot::new(home.keep()).unwrap();
        let env_name = EnvironmentName::from_str("test").unwrap();
        let env_dir = EnvDir::from_env_root(env_root, &env_name).await.unwrap();

        // Test empty
        let specs = IndexSet::new();
        let prefix = Prefix::new(env_dir.path());
        let prefix_records = prefix.find_installed_packages().unwrap();
        let result = environment_specs_in_sync(&prefix_records, &specs, &HashSet::new(), None)
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
        let test_data_source = PathBuf::from(env!("CARGO_WORKSPACE_DIR"))
            .join("crates/pixi_global/src/test_data/conda-meta")
            .join(file_name);
        tokio_fs::copy(test_data_source, test_data_target)
            .await
            .unwrap();

        let prefix_records = prefix.find_installed_packages().unwrap();
        let result = environment_specs_in_sync(&prefix_records, &specs, &HashSet::new(), None)
            .await
            .unwrap();
        assert!(result);
    }
}
