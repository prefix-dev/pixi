//! ROS backend configuration.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use indexmap::IndexMap;
use pixi_build_backend::generated_recipe::BackendConfig;
use rattler_conda_types::ChannelUrl;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::package_map::PackageMapEntry;

/// Configuration for the ROS build backend.
#[derive(Debug, Default, Deserialize, Serialize, Clone)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RosBackendConfig {
    /// ROS distribution name (e.g., "humble", "jazzy", "noetic").
    /// If not set, auto-detected from robostack channel names.
    pub distro: Option<String>,

    /// Whether to build a noarch package.
    pub noarch: Option<bool>,

    /// Environment variables to set during the build.
    #[serde(default)]
    pub env: Option<IndexMap<String, String>>,

    /// Deprecated. Debug data is always written to the work directory.
    pub debug_dir: Option<PathBuf>,

    /// Extra input globs for build cache invalidation.
    #[serde(default)]
    pub extra_input_globs: Option<Vec<String>>,

    /// Extra package mapping sources.
    #[serde(default)]
    pub extra_package_mappings: Vec<PackageMappingSource>,
}

impl RosBackendConfig {
    /// Get file paths from all package mapping sources that came from files.
    pub fn get_package_mapping_file_paths(&self) -> Vec<PathBuf> {
        self.extra_package_mappings
            .iter()
            .filter_map(|source| match source {
                PackageMappingSource::File { path } => Some(path.clone()),
                PackageMappingSource::Mapping(_) => None,
            })
            .collect()
    }
}

impl BackendConfig for RosBackendConfig {
    fn debug_dir(&self) -> Option<&Path> {
        self.debug_dir.as_deref()
    }

    fn merge_with_target_config(&self, target_config: &Self) -> miette::Result<Self> {
        Ok(Self {
            distro: target_config.distro.clone().or_else(|| self.distro.clone()),
            noarch: target_config.noarch.or(self.noarch),
            env: match (&self.env, &target_config.env) {
                (Some(base), Some(target)) => {
                    let mut merged = base.clone();
                    merged.extend(target.clone());
                    Some(merged)
                }
                (None, Some(target)) => Some(target.clone()),
                (base, None) => base.clone(),
            },
            debug_dir: self.debug_dir.clone(),
            extra_input_globs: if target_config.extra_input_globs.is_some() {
                target_config.extra_input_globs.clone()
            } else {
                self.extra_input_globs.clone()
            },
            extra_package_mappings: if target_config.extra_package_mappings.is_empty() {
                self.extra_package_mappings.clone()
            } else {
                target_config.extra_package_mappings.clone()
            },
        })
    }
}

/// Describes where additional package mapping data comes from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PackageMappingSource {
    /// A file path to a YAML mapping file.
    File { path: PathBuf },
    /// An inline mapping dictionary.
    Mapping(HashMap<String, PackageMapEntry>),
}

/// Extract ROS distro name from channel URLs.
///
/// Looks for channels matching `robostack-<distro>` (excluding
/// `robostack-staging`).
pub fn extract_distro_from_channels_list(channels: &[ChannelUrl]) -> Option<String> {
    static PATTERN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"robostack-(\w+)").expect("valid regex"));

    for channel in channels {
        let channel_str = channel.as_str();
        let channel_name = channel_str
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(channel_str);
        if let Some(caps) = PATTERN.captures(channel_name) {
            let distro = caps.get(1).map(|m| m.as_str().to_string())?;
            if distro == "staging" {
                continue;
            }
            return Some(distro);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::*;

    fn channel(url: &str) -> ChannelUrl {
        ChannelUrl::from(Url::parse(url).unwrap())
    }

    #[test]
    fn test_extract_distro_from_full_url() {
        let channels = vec![
            channel("https://prefix.dev/pixi-build-backends"),
            channel("https://prefix.dev/robostack-jazzy"),
            channel("https://prefix.dev/conda-forge"),
        ];
        assert_eq!(
            extract_distro_from_channels_list(&channels),
            Some("jazzy".to_string())
        );
    }

    #[test]
    fn test_extract_distro_from_short_channel_name() {
        let channels = vec![
            channel("https://prefix.dev/robostack-humble"),
            channel("https://prefix.dev/conda-forge"),
        ];
        assert_eq!(
            extract_distro_from_channels_list(&channels),
            Some("humble".to_string())
        );
    }

    #[test]
    fn test_dont_extract_from_staging() {
        let channels = vec![
            channel("https://prefix.dev/robostack-staging"),
            channel("https://prefix.dev/conda-forge"),
        ];
        assert_eq!(extract_distro_from_channels_list(&channels), None);
    }

    #[test]
    fn test_extract_distro_with_trailing_slash() {
        let channels = vec![channel("https://prefix.dev/robostack-noetic/")];
        assert_eq!(
            extract_distro_from_channels_list(&channels),
            Some("noetic".to_string())
        );
    }

    #[test]
    fn test_extract_distro_multiple_robostack_channels() {
        let channels = vec![
            channel("https://prefix.dev/robostack-humble"),
            channel("https://prefix.dev/robostack-jazzy"),
        ];
        assert_eq!(
            extract_distro_from_channels_list(&channels),
            Some("humble".to_string())
        );
    }

    #[test]
    fn test_extract_distro_no_robostack_channel() {
        let channels = vec![
            channel("https://prefix.dev/conda-forge"),
            channel("https://prefix.dev/some-other-channel"),
        ];
        assert_eq!(extract_distro_from_channels_list(&channels), None);
    }

    #[test]
    fn test_ensure_deserialize_from_empty() {
        let json_data = serde_json::json!({});
        serde_json::from_value::<RosBackendConfig>(json_data).unwrap();
    }

    #[test]
    fn test_merge_with_target_config() {
        let base = RosBackendConfig {
            distro: Some("humble".to_string()),
            env: Some(IndexMap::from([(
                "BASE_VAR".to_string(),
                "base_value".to_string(),
            )])),
            debug_dir: Some(PathBuf::from("/base/debug")),
            ..Default::default()
        };

        let target = RosBackendConfig {
            env: Some(IndexMap::from([(
                "TARGET_VAR".to_string(),
                "target_value".to_string(),
            )])),
            ..Default::default()
        };

        let merged = base.merge_with_target_config(&target).unwrap();
        assert_eq!(merged.distro.as_deref(), Some("humble"));
        assert_eq!(
            merged.env.as_ref().unwrap().get("BASE_VAR"),
            Some(&"base_value".to_string())
        );
        assert_eq!(
            merged.env.as_ref().unwrap().get("TARGET_VAR"),
            Some(&"target_value".to_string())
        );
    }
}
