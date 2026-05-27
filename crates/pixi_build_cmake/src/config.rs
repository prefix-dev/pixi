use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use pixi_build_backend::generated_recipe::BackendConfig;
use serde::{Deserialize, Serialize};

/// The compiler cache to use during builds.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CompilerCache {
    /// Use sccache as the compiler cache.
    Sccache,
}

/// A `compiler-cache` setting together with where it came from.
///
/// The two forms are deserialized from the same `compiler-cache` key but carry
/// different consequences, so the build can keep the lockfile deterministic:
///
/// - [`Self::Package`] — written by the package itself as a bare string
///   (`compiler-cache = "sccache"`). The cache tool is added to the build
///   requirements and therefore captured in the lockfile.
/// - [`Self::Default`] — injected by the command dispatcher as
///   `{ "default": "sccache" }` from the user's global/project pixi config. As
///   a per-machine preference it is used as a compiler launcher only and is
///   never added to the locked build requirements, so the tool must already be
///   on `PATH`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum CompilerCacheConfig {
    /// Set in the package manifest; locked as a build dependency.
    Package(CompilerCache),
    /// Injected default from pixi config; launcher only, not locked.
    Default {
        /// The cache requested by the global/project config.
        default: CompilerCache,
    },
}

impl CompilerCacheConfig {
    /// The requested cache tool, regardless of where the setting came from.
    pub fn cache(&self) -> &CompilerCache {
        match self {
            Self::Package(cache) | Self::Default { default: cache } => cache,
        }
    }

    /// Whether the cache tool should be added to the locked build
    /// requirements. Only a package-local setting is locked; an injected
    /// per-machine default is used as a launcher only.
    pub fn lock_as_dependency(&self) -> bool {
        matches!(self, Self::Package(_))
    }
}

#[derive(Debug, Default, Deserialize, Serialize, Clone)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct CMakeBackendConfig {
    /// Extra args for CMake invocation
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// System environment variables (populated at runtime, not serialized)
    #[serde(skip)]
    pub system_env: IndexMap<String, String>,
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
    /// List of compilers to use (e.g., ["c", "cxx", "cuda"])
    /// If not specified, a default will be used
    pub compilers: Option<Vec<String>>,
    /// The compiler cache to use. A bare `compiler-cache = "sccache"` in the
    /// package manifest is locked as a build dependency; a value injected from
    /// the user's pixi config is used as a launcher only. See
    /// [`CompilerCacheConfig`].
    pub compiler_cache: Option<CompilerCacheConfig>,
}

fn collect_system_env() -> IndexMap<String, String> {
    std::env::vars().collect()
}

impl CMakeBackendConfig {
    /// Create a new `CMakeBackendConfig` with the current system environment.
    pub fn new_with_system_environment() -> Self {
        Self {
            system_env: collect_system_env(),
            ..Self::default()
        }
    }
}

impl BackendConfig for CMakeBackendConfig {
    fn debug_dir(&self) -> Option<&Path> {
        self.debug_dir.as_deref()
    }

    /// Merge this configuration with a target-specific configuration.
    /// Target-specific values override base values using the following rules:
    /// - extra_args: Platform-specific completely replaces base
    /// - env: Platform env vars override base, others merge
    /// - extra_input_globs: Platform-specific completely replaces base
    /// - compilers: Platform-specific completely replaces base
    fn merge_with_target_config(&self, target_config: &Self) -> miette::Result<Self> {
        Ok(Self {
            extra_args: if target_config.extra_args.is_empty() {
                self.extra_args.clone()
            } else {
                target_config.extra_args.clone()
            },
            system_env: collect_system_env(),
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
            compiler_cache: target_config
                .compiler_cache
                .clone()
                .or_else(|| self.compiler_cache.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use pixi_build_backend::generated_recipe::BackendConfig;
    use serde_json::json;
    use std::path::PathBuf;

    use super::{CMakeBackendConfig, CompilerCache, CompilerCacheConfig};

    #[test]
    fn test_ensure_deserialize_from_empty() {
        let json_data = json!({});
        serde_json::from_value::<CMakeBackendConfig>(json_data).unwrap();
    }

    #[test]
    fn test_compiler_cache_distinguishes_package_from_injected_default() {
        // A bare string is what a package writes in its manifest: locked.
        let package = serde_json::from_value::<CMakeBackendConfig>(json!({
            "compiler-cache": "sccache"
        }))
        .unwrap()
        .compiler_cache
        .unwrap();
        assert_eq!(package, CompilerCacheConfig::Package(CompilerCache::Sccache));
        assert!(package.lock_as_dependency());

        // The tagged form is what the command dispatcher injects from global
        // config: a launcher only, never locked.
        let injected = serde_json::from_value::<CMakeBackendConfig>(json!({
            "compiler-cache": { "default": "sccache" }
        }))
        .unwrap()
        .compiler_cache
        .unwrap();
        assert_eq!(
            injected,
            CompilerCacheConfig::Default {
                default: CompilerCache::Sccache
            }
        );
        assert!(!injected.lock_as_dependency());

        // Both still resolve to the same underlying cache tool.
        assert_eq!(package.cache(), injected.cache());
    }

    #[test]
    fn test_merge_with_target_config() {
        let mut base_env = indexmap::IndexMap::new();
        base_env.insert("BASE_VAR".to_string(), "base_value".to_string());
        base_env.insert("SHARED_VAR".to_string(), "base_shared".to_string());

        let base_config = CMakeBackendConfig {
            extra_args: vec!["--base-arg".to_string()],
            env: base_env,
            debug_dir: Some(PathBuf::from("/base/debug")),
            extra_input_globs: vec!["*.base".to_string()],
            compilers: Some(vec!["cxx".to_string()]),
            ..CMakeBackendConfig::default()
        };

        let mut target_env = indexmap::IndexMap::new();
        target_env.insert("TARGET_VAR".to_string(), "target_value".to_string());
        target_env.insert("SHARED_VAR".to_string(), "target_shared".to_string());

        let target_config = CMakeBackendConfig {
            extra_args: vec!["--target-arg".to_string()],
            env: target_env,
            debug_dir: None,
            extra_input_globs: vec!["*.target".to_string()],
            compilers: Some(vec!["c".to_string(), "cuda".to_string()]),
            ..CMakeBackendConfig::default()
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
            Some(vec!["c".to_string(), "cuda".to_string()])
        );
    }

    #[test]
    fn test_merge_with_empty_target_config() {
        let mut base_env = indexmap::IndexMap::new();
        base_env.insert("BASE_VAR".to_string(), "base_value".to_string());

        let base_config = CMakeBackendConfig {
            extra_args: vec!["--base-arg".to_string()],
            env: base_env,
            debug_dir: Some(PathBuf::from("/base/debug")),
            extra_input_globs: vec!["*.base".to_string()],
            compilers: Some(vec!["cxx".to_string()]),
            ..CMakeBackendConfig::default()
        };

        let empty_target_config = CMakeBackendConfig::default();

        let merged = base_config
            .merge_with_target_config(&empty_target_config)
            .unwrap();

        // Should keep base values when target is empty
        assert_eq!(merged.extra_args, vec!["--base-arg".to_string()]);
        assert_eq!(merged.env.get("BASE_VAR"), Some(&"base_value".to_string()));
        assert_eq!(merged.debug_dir, Some(PathBuf::from("/base/debug")));
        assert_eq!(merged.extra_input_globs, vec!["*.base".to_string()]);
        assert_eq!(merged.compilers, Some(vec!["cxx".to_string()]));
    }
}
