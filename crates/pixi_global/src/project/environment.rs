use crate::install::local_environment_matches_spec;
use console::StyledObject;
use fancy_display::FancyDisplay;
use indexmap::{IndexMap, IndexSet};
use is_executable::IsExecutable;
use miette::{Diagnostic, IntoDiagnostic};
use pixi_consts::consts;
use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName};
use pixi_utils::is_binary_folder;
use pixi_utils::prefix::{Executable, Prefix};
use pixi_utils::strip_executable_extension;
use rattler::install::PythonInfo;
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

/// A PyPI distribution found in a prefix's site-packages.
pub(crate) struct InstalledPypiDistribution {
    /// The distribution name as found in the `.dist-info` directory name,
    /// lowercased. Following the wheel spec this uses `_` where
    /// pep508-normalized names use `-`.
    pub dist_info_name: String,
    /// True when the distribution was installed by pixi (rather than e.g.
    /// pip run by the user inside the environment).
    pub pixi_installed: bool,
    /// The path of the `.dist-info` directory.
    pub dist_info_path: std::path::PathBuf,
}

/// Scans `site_packages` for installed distributions by looking at the
/// `{name}-{version}.dist-info` directories.
pub(crate) fn installed_pypi_distributions(
    site_packages: &std::path::Path,
) -> Vec<InstalledPypiDistribution> {
    let mut result = Vec::new();
    if let Ok(entries) = fs_err::read_dir(site_packages) {
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(dir_name) = file_name.to_str() else {
                continue;
            };
            if let Some(dist) = dir_name.strip_suffix(".dist-info")
                && let Some((name, _version)) = dist.split_once('-')
            {
                let pixi_installed = fs_err::read_to_string(entry.path().join("INSTALLER"))
                    .map(|installer| installer.trim() == consts::PIXI_UV_INSTALLER)
                    .unwrap_or(false);
                result.push(InstalledPypiDistribution {
                    dist_info_name: name.to_lowercase(),
                    pixi_installed,
                    dist_info_path: entry.path(),
                });
            }
        }
    }
    result
}

/// Converts a pep508-normalized package name to the spelling used in
/// `.dist-info` directory names, where runs of `-_.` are replaced by `_`.
pub(crate) fn dist_info_name(name: &PypiPackageName) -> String {
    name.as_normalized().to_string().replace('-', "_")
}

/// Extracts the path field from a line in a `.dist-info/RECORD` file. The
/// format is CSV with three fields, `path,hash,size`, where the path is
/// quoted when it contains special characters.
fn record_entry_path(line: &str) -> Option<String> {
    if let Some(rest) = line.strip_prefix('"') {
        let mut path = String::new();
        let mut chars = rest.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    path.push('"');
                    chars.next();
                } else {
                    break;
                }
            } else {
                path.push(c);
            }
        }
        Some(path)
    } else {
        line.split(',')
            .next()
            .map(str::to_string)
            .filter(|path| !path.is_empty())
    }
}

/// Logically normalizes a path by resolving `.` and `..` components.
fn normalize_path(path: &std::path::Path) -> std::path::PathBuf {
    let mut result = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                result.pop();
            }
            other => result.push(other),
        }
    }
    result
}

/// Returns the executables a PyPI distribution installed into the prefix's
/// binary folders, based on the `RECORD` of its `.dist-info` directory.
pub(crate) fn pypi_distribution_executables(
    prefix: &Prefix,
    site_packages: &std::path::Path,
    dist_info_path: &std::path::Path,
) -> Vec<Executable> {
    let Ok(record) = fs_err::read_to_string(dist_info_path.join("RECORD")) else {
        return Vec::new();
    };

    let mut executables = Vec::new();
    for line in record.lines() {
        let Some(path) = record_entry_path(line) else {
            continue;
        };
        // Paths in RECORD are relative to site-packages; scripts use
        // `../../../bin/...` style paths to reach the prefix's binary folder.
        let absolute = normalize_path(&site_packages.join(path));
        let Ok(relative) = absolute.strip_prefix(prefix.root()) else {
            continue;
        };
        let Some(parent) = relative.parent() else {
            continue;
        };
        if !is_binary_folder(parent) || !absolute.is_executable() {
            continue;
        }
        if let Some(name) = relative.file_name().and_then(|name| name.to_str()) {
            executables.push(Executable::new(
                strip_executable_extension(name.to_string()),
                relative.to_path_buf(),
            ));
        }
    }
    executables
}

/// Returns the executables of the pixi-installed PyPI distributions in
/// `site_packages`. When `only_dists` is given, only distributions whose
/// (dist-info spelled) name is in the set are considered.
pub(crate) fn pypi_executables(
    prefix: &Prefix,
    site_packages: &std::path::Path,
    only_dists: Option<&HashSet<String>>,
) -> Vec<Executable> {
    installed_pypi_distributions(site_packages)
        .into_iter()
        .filter(|dist| dist.pixi_installed)
        .filter(|dist| only_dists.is_none_or(|names| names.contains(&dist.dist_info_name)))
        .flat_map(|dist| pypi_distribution_executables(prefix, site_packages, &dist.dist_info_path))
        .collect()
}

/// Returns the site-packages directory of the prefix, if it contains a
/// python interpreter.
pub(crate) fn find_site_packages(
    python_record: Option<&rattler_conda_types::PackageRecord>,
    prefix: &Prefix,
    platform: Platform,
) -> miette::Result<Option<std::path::PathBuf>> {
    let Some(python_record) = python_record else {
        return Ok(None);
    };
    let python_info = PythonInfo::from_python_record(python_record, platform).into_diagnostic()?;
    Ok(Some(prefix.root().join(&python_info.site_packages_path)))
}

/// Checks whether the PyPI packages declared in the manifest match what is
/// present in the prefix's site-packages.
///
/// This is a name-presence check only: it detects added or removed
/// pypi-dependencies (including pixi-installed leftovers that should be
/// removed), but a changed version requirement for an already-installed
/// package does not mark the environment as out of sync. The PyPI installer
/// reconciles versions the next time the environment is (re)installed.
pub(crate) fn pypi_dependencies_in_sync(
    pypi_dependencies: &IndexMap<PypiPackageName, PixiPypiSpec>,
    prefix_records: &[PrefixRecord],
    prefix: &Prefix,
    platform: Platform,
) -> miette::Result<bool> {
    // Locate the python interpreter among the installed conda packages; its
    // record determines where site-packages lives.
    let python_record = prefix_records
        .iter()
        .map(|r| &r.repodata_record.package_record)
        .find(|r| r.name.as_normalized() == "python");
    let Some(site_packages) = find_site_packages(python_record, prefix, platform)? else {
        // PyPI packages cannot be installed without an interpreter; trigger a
        // sync so installation can surface a proper error.
        return Ok(pypi_dependencies.is_empty());
    };

    let installed = installed_pypi_distributions(&site_packages);

    // When nothing is declared anymore, any distribution previously
    // installed by pixi has to be removed.
    if pypi_dependencies.is_empty() {
        return Ok(!installed.iter().any(|dist| dist.pixi_installed));
    }

    let installed_names: HashSet<&str> = installed
        .iter()
        .map(|dist| dist.dist_info_name.as_str())
        .collect();
    Ok(pypi_dependencies
        .keys()
        .all(|name| installed_names.contains(dist_info_name(name).as_str())))
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
