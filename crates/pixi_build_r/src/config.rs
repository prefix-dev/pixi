use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use pixi_build_backend::generated_recipe::BackendConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize, Serialize, Clone)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RBackendConfig {
    /// Extra args for R CMD INSTALL invocation
    /// Example: ["--no-multiarch", "--no-test-load"]
    #[serde(default)]
    pub extra_args: Vec<String>,

    /// Environment Variables
    #[serde(default)]
    pub env: IndexMap<String, String>,

    /// Deprecated. Setting this has no effect; debug data is always written to
    /// the `debug` subdirectory of the work directory.
    #[serde(alias = "debug_dir")]
    pub debug_dir: Option<PathBuf>,

    /// Extra input globs to include in addition to the default ones
    #[serde(default)]
    pub extra_input_globs: Vec<String>,

    /// List of compilers to use (e.g., ["c", "cxx", "fortran"])
    /// If not specified, will be auto-detected by checking for src/ directory
    /// and LinkingTo field in DESCRIPTION
    pub compilers: Option<Vec<String>>,

    /// Channel list for R package dependencies
    /// Defaults to ["conda-forge"] if not specified
    #[serde(default)]
    pub channels: Option<Vec<String>>,
}

impl BackendConfig for RBackendConfig {
    fn debug_dir(&self) -> Option<&Path> {
        self.debug_dir.as_deref()
    }

    fn merge_with_target_config(&self, target_config: &Self) -> miette::Result<Self> {
        if target_config.debug_dir.is_some() {
            miette::bail!("`debug_dir` cannot have a target specific value");
        }

        Ok(Self {
            extra_args: if target_config.extra_args.is_empty() {
                self.extra_args.clone()
            } else {
                target_config.extra_args.clone()
            },
            env: {
                let mut merged_env = self.env.clone();
                merged_env.extend(target_config.env.clone());
                merged_env
            },
            debug_dir: self.debug_dir.clone(),
            extra_input_globs: if target_config.extra_input_globs.is_empty() {
                self.extra_input_globs.clone()
            } else {
                target_config.extra_input_globs.clone()
            },
            compilers: target_config
                .compilers
                .clone()
                .or_else(|| self.compilers.clone()),
            channels: target_config
                .channels
                .clone()
                .or_else(|| self.channels.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_deserialize_from_empty() {
        let json_data = json!({});
        let config: RBackendConfig = serde_json::from_value(json_data).unwrap();
        assert!(config.extra_args.is_empty());
        assert!(config.env.is_empty());
        assert!(config.compilers.is_none());
    }

    #[test]
    fn test_deserialize_full_config() {
        let json_data = json!({
            "extra-args": ["--no-multiarch", "--no-test-load"],
            "env": {"R_LIBS_USER": "$PREFIX/lib/R/library"},
            "extra-input-globs": ["inst/**/*"],
            "compilers": ["c", "cxx"],
            "channels": ["conda-forge", "r"]
        });
        let config: RBackendConfig = serde_json::from_value(json_data).unwrap();
        assert_eq!(config.extra_args, vec!["--no-multiarch", "--no-test-load"]);
        assert_eq!(config.env.len(), 1);
        assert_eq!(
            config.compilers,
            Some(vec!["c".to_string(), "cxx".to_string()])
        );
        assert_eq!(
            config.channels,
            Some(vec!["conda-forge".to_string(), "r".to_string()])
        );
    }

    #[test]
    fn test_merge_with_target_config() {
        let base_config = RBackendConfig {
            extra_args: vec!["--no-lock".to_string()],
            env: IndexMap::from([("BASE_VAR".to_string(), "base".to_string())]),
            debug_dir: None,
            extra_input_globs: vec!["**/*.R".to_string()],
            compilers: Some(vec!["c".to_string()]),
            channels: Some(vec!["conda-forge".to_string()]),
        };

        let target_config = RBackendConfig {
            extra_args: vec!["--no-multiarch".to_string()],
            env: IndexMap::from([("TARGET_VAR".to_string(), "target".to_string())]),
            debug_dir: None,
            extra_input_globs: vec![],
            compilers: Some(vec!["c".to_string(), "cxx".to_string()]),
            channels: None,
        };

        let merged = base_config
            .merge_with_target_config(&target_config)
            .unwrap();

        // Target extra_args should replace base
        assert_eq!(merged.extra_args, vec!["--no-multiarch"]);

        // Env should be merged with target taking precedence
        assert_eq!(merged.env.len(), 2);
        assert_eq!(merged.env.get("BASE_VAR").unwrap(), "base");
        assert_eq!(merged.env.get("TARGET_VAR").unwrap(), "target");

        // Target compilers should override
        assert_eq!(
            merged.compilers,
            Some(vec!["c".to_string(), "cxx".to_string()])
        );

        // Base channels should be used (target is None)
        assert_eq!(merged.channels, Some(vec!["conda-forge".to_string()]));
    }

    #[test]
    fn test_debug_dir_target_specific_error() {
        let base_config = RBackendConfig::default();
        let target_config = RBackendConfig {
            debug_dir: Some(PathBuf::from("/some/path")),
            ..Default::default()
        };

        let result = base_config.merge_with_target_config(&target_config);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("debug_dir` cannot have a target specific value")
        );
    }
}
