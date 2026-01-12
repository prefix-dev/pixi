use indexmap::IndexMap;
use pixi_build_backend::generated_recipe::BackendConfig;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RustBackendConfig {
    /// Extra args to pass for cargo
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
    /// Ignore the cargo manifest and depend only on the project model.
    #[serde(default)]
    pub ignore_cargo_manifest: Option<bool>,
    /// List of compilers to use (e.g., ["rust", "c", "cxx"])
    /// If not specified, a default will be used
    pub compilers: Option<Vec<String>>,
}

impl RustBackendConfig {
    /// Creates a new [`RustBackendConfig`] with default values and
    /// `ignore_cargo_manifest` set to `true`.
    #[cfg(test)]
    pub fn default_with_ignore_cargo_manifest() -> Self {
        Self {
            ignore_cargo_manifest: Some(true),
            ..Default::default()
        }
    }
}

impl BackendConfig for RustBackendConfig {
    fn debug_dir(&self) -> Option<&Path> {
        self.debug_dir.as_deref()
    }

    /// Merge this configuration with a target-specific configuration.
    /// Target-specific values override base values using the following rules:
    /// - extra_args: Platform-specific completely replaces base
    /// - env: Platform env vars override base, others merge
    /// - debug_dir: Not allowed to have target specific value
    /// - extra_input_globs: Platform-specific completely replaces base
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
            ignore_cargo_manifest: target_config
                .ignore_cargo_manifest
                .or(self.ignore_cargo_manifest),
            compilers: target_config
                .compilers
                .clone()
                .or_else(|| self.compilers.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::RustBackendConfig;
    use pixi_build_backend::generated_recipe::BackendConfig;
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn test_ensure_deseralize_from_empty() {
        let json_data = json!({});
        serde_json::from_value::<RustBackendConfig>(json_data).unwrap();
    }

    #[test]
    fn test_debug_dir_accepts_both_formats() {
        // Test with debug-dir (kebab-case) - the canonical format
        let json_with_hyphen = json!({
            "debug-dir": "/path/to/debug"
        });
        let config = serde_json::from_value::<RustBackendConfig>(json_with_hyphen).unwrap();
        assert_eq!(config.debug_dir, Some(PathBuf::from("/path/to/debug")));

        // Test with debug_dir (underscore) - should also work due to alias
        let json_with_underscore = json!({
            "debug_dir": "/path/to/debug"
        });
        let config = serde_json::from_value::<RustBackendConfig>(json_with_underscore).unwrap();
        assert_eq!(config.debug_dir, Some(PathBuf::from("/path/to/debug")));
    }

    #[test]
    fn test_merge_with_target_config() {
        let mut base_env = indexmap::IndexMap::new();
        base_env.insert("BASE_VAR".to_string(), "base_value".to_string());
        base_env.insert("SHARED_VAR".to_string(), "base_shared".to_string());

        let base_config = RustBackendConfig {
            extra_args: vec!["--base-arg".to_string()],
            env: base_env,
            debug_dir: Some(PathBuf::from("/base/debug")),
            extra_input_globs: vec!["*.base".to_string()],
            ignore_cargo_manifest: None,
            compilers: Some(vec!["rust".to_string()]),
        };

        let mut target_env = indexmap::IndexMap::new();
        target_env.insert("TARGET_VAR".to_string(), "target_value".to_string());
        target_env.insert("SHARED_VAR".to_string(), "target_shared".to_string());

        let target_config = RustBackendConfig {
            extra_args: vec!["--target-arg".to_string()],
            env: target_env,
            debug_dir: None,
            extra_input_globs: vec!["*.target".to_string()],
            ignore_cargo_manifest: Some(true),
            compilers: Some(vec!["c".to_string(), "rust".to_string()]),
        };

        let merged = base_config
            .merge_with_target_config(&target_config)
            .unwrap();

        // extra_args should be completely overridden
        assert_eq!(merged.extra_args, vec!["--target-arg".to_string()]);

        // env should merge with target taking precedence
        assert_eq!(merged.env.get("BASE_VAR"), Some(&"base_value".to_string()));
        assert_eq!(
            merged.env.get("TARGET_VAR"),
            Some(&"target_value".to_string())
        );
        assert_eq!(
            merged.env.get("SHARED_VAR"),
            Some(&"target_shared".to_string())
        );

        // debug_dir should use base value
        assert_eq!(merged.debug_dir, Some(PathBuf::from("/base/debug")));

        // extra_input_globs should be completely overridden
        assert_eq!(merged.extra_input_globs, vec!["*.target".to_string()]);

        // compilers should be completely overridden by target
        assert_eq!(
            merged.compilers,
            Some(vec!["c".to_string(), "rust".to_string()])
        );
    }

    #[test]
    fn test_merge_with_empty_target_config() {
        let mut base_env = indexmap::IndexMap::new();
        base_env.insert("BASE_VAR".to_string(), "base_value".to_string());

        let base_config = RustBackendConfig {
            extra_args: vec!["--base-arg".to_string()],
            env: base_env,
            debug_dir: Some(PathBuf::from("/base/debug")),
            extra_input_globs: vec!["*.base".to_string()],
            ignore_cargo_manifest: None,
            compilers: Some(vec!["rust".to_string()]),
        };

        let empty_target_config = RustBackendConfig::default();

        let merged = base_config
            .merge_with_target_config(&empty_target_config)
            .unwrap();

        // Should keep base values when target is empty
        assert_eq!(merged.extra_args, vec!["--base-arg".to_string()]);
        assert_eq!(merged.env.get("BASE_VAR"), Some(&"base_value".to_string()));
        assert_eq!(merged.debug_dir, Some(PathBuf::from("/base/debug")));
        assert_eq!(merged.extra_input_globs, vec!["*.base".to_string()]);
        assert_eq!(merged.compilers, Some(vec!["rust".to_string()]));
    }

    #[test]
    fn test_merge_target_debug_dir_error() {
        let base_config = RustBackendConfig {
            debug_dir: Some(PathBuf::from("/base/debug")),
            ..Default::default()
        };

        let target_config = RustBackendConfig {
            debug_dir: Some(PathBuf::from("/target/debug")),
            ..Default::default()
        };

        let result = base_config.merge_with_target_config(&target_config);
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("`debug_dir` cannot have a target specific value"));
    }
}
