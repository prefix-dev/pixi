use indexmap::IndexMap;
use pixi_build_backend::generated_recipe::BackendConfig;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One value of the `pypi-conda-map` option: a conda package name, or `false`
/// to silently drop the dependency from the generated recipe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PypiCondaMapEntry {
    /// Map the PyPI package to this conda package name.
    CondaName(String),
    /// Drop the dependency from the generated recipe.
    Skip,
}

impl Serialize for PypiCondaMapEntry {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            PypiCondaMapEntry::CondaName(name) => serializer.serialize_str(name),
            PypiCondaMapEntry::Skip => serializer.serialize_bool(false),
        }
    }
}

impl<'de> Deserialize<'de> for PypiCondaMapEntry {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = PypiCondaMapEntry;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a conda package name or `false`")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(PypiCondaMapEntry::CondaName(v.to_string()))
            }

            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
                if v {
                    Err(E::custom(
                        "`true` is not supported; use a conda package name to map the \
                         package, or `false` to drop the dependency",
                    ))
                } else {
                    Ok(PypiCondaMapEntry::Skip)
                }
            }

            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(PypiCondaMapEntry::Skip)
            }

            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(PypiCondaMapEntry::Skip)
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

/// Represents skip-pyc-compilation config: either `true` (skip all) or a list
/// of glob patterns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SkipPycCompilation {
    All(bool),
    Globs(Vec<String>),
}

impl Default for SkipPycCompilation {
    fn default() -> Self {
        SkipPycCompilation::All(false)
    }
}

impl SkipPycCompilation {
    pub fn globs(&self) -> Vec<String> {
        match self {
            SkipPycCompilation::All(true) => vec!["**/*.py".to_string()],
            SkipPycCompilation::All(false) => vec![],
            SkipPycCompilation::Globs(g) => g.clone(),
        }
    }

    pub fn is_none(&self) -> bool {
        self.globs().is_empty()
    }
}

#[derive(Debug, Default, Deserialize, Serialize, Clone)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PythonBackendConfig {
    /// True if the package should be build as a python noarch package. Defaults
    /// to `true`.
    #[serde(default)]
    pub noarch: Option<bool>,
    /// Extra args to pass to pip
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
    /// List of compilers to use (e.g., ["c", "cxx", "rust"])
    /// If not specified, no compilers are added (since most Python packages are pure Python)
    pub compilers: Option<Vec<String>>,
    /// Ignore the pyproject.toml manifest and rely only on the project model.
    #[serde(default)]
    pub ignore_pyproject_manifest: Option<bool>,
    /// Ignore the PyPI-to-conda mapping. When enabled, dependencies from
    /// pyproject.toml will not be automatically mapped to conda packages.
    /// Defaults to `true` (mapping disabled).
    #[serde(default)]
    pub ignore_pypi_mapping: Option<bool>,
    /// User-defined overrides for the PyPI-to-conda name mapping, keyed by
    /// PyPI package name. A string value maps the package to that conda name;
    /// `false` drops the dependency from the generated recipe. Entries are
    /// consulted before the remote mapping service. Only used when
    /// `ignore-pypi-mapping = false`.
    #[serde(default)]
    pub pypi_conda_map: Option<IndexMap<String, PypiCondaMapEntry>>,
    /// Whether the package uses the Python Stable ABI (abi3).
    /// When true, adds `python_abi` to host requirements.
    /// Only meaningful for packages with compiled extensions (non-noarch).
    #[serde(default)]
    pub abi3: Option<bool>,
    /// Skip .pyc compilation for matching files. Accepts `true` to skip all
    /// .pyc compilation, or a list of glob patterns (e.g. `["tests/**"]`).
    #[serde(default)]
    pub skip_pyc_compilation: SkipPycCompilation,
}

impl PythonBackendConfig {
    /// Whether to build a noarch package or a platform-specific package.
    pub fn noarch(&self) -> bool {
        self.noarch.unwrap_or(true)
    }

    /// Whether to ignore the PyPI-to-conda mapping.
    /// Defaults to `true` (mapping disabled).
    pub fn ignore_pypi_mapping(&self) -> bool {
        self.ignore_pypi_mapping.unwrap_or(true)
    }

    /// Creates a new [`PythonBackendConfig`] with default values and
    /// `ignore_pyproject_manifest` set to `true`.
    #[cfg(test)]
    pub fn default_with_ignore_pyproject_manifest() -> Self {
        Self {
            ignore_pyproject_manifest: Some(true),
            ..Default::default()
        }
    }
}

impl BackendConfig for PythonBackendConfig {
    fn debug_dir(&self) -> Option<&Path> {
        self.debug_dir.as_deref()
    }

    /// Merge this configuration with a target-specific configuration.
    /// Target-specific values override base values using the following rules:
    /// - noarch: Platform-specific takes precedence (critical for cross-platform)
    /// - env: Platform env vars override base, others merge
    /// - extra_args: Platform-specific completely replaces base
    /// - extra_input_globs: Platform-specific completely replaces base
    fn merge_with_target_config(&self, target_config: &Self) -> miette::Result<Self> {
        Ok(Self {
            noarch: target_config.noarch.or(self.noarch),
            env: {
                let mut merged_env = self.env.clone();
                merged_env.extend(target_config.env.clone());
                merged_env
            },
            debug_dir: self.debug_dir.clone(),
            extra_args: if target_config.extra_args.is_empty() {
                self.extra_args.clone()
            } else {
                target_config.extra_args.clone()
            },
            extra_input_globs: if target_config.extra_input_globs.is_empty() {
                self.extra_input_globs.clone()
            } else {
                target_config.extra_input_globs.clone()
            },
            compilers: target_config
                .compilers
                .clone()
                .or_else(|| self.compilers.clone()),
            ignore_pyproject_manifest: target_config
                .ignore_pyproject_manifest
                .or(self.ignore_pyproject_manifest),
            ignore_pypi_mapping: target_config
                .ignore_pypi_mapping
                .or(self.ignore_pypi_mapping),
            pypi_conda_map: match (&self.pypi_conda_map, &target_config.pypi_conda_map) {
                (None, None) => None,
                (base, target) => {
                    // Per-key merge: target entries override or add to the base map.
                    let mut merged = base.clone().unwrap_or_default();
                    merged.extend(target.clone().unwrap_or_default());
                    Some(merged)
                }
            },
            abi3: target_config.abi3.or(self.abi3),
            skip_pyc_compilation: if target_config.skip_pyc_compilation.is_none() {
                self.skip_pyc_compilation.clone()
            } else {
                target_config.skip_pyc_compilation.clone()
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{PypiCondaMapEntry, PythonBackendConfig, SkipPycCompilation};
    use pixi_build_backend::generated_recipe::BackendConfig;
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn test_ensure_deserialize_from_empty() {
        let json_data = json!({});
        serde_json::from_value::<PythonBackendConfig>(json_data).unwrap();
    }

    #[test]
    fn test_deserialize_pypi_conda_map() {
        let json_data = json!({
            "pypi-conda-map": {
                "torch": "pytorch",
                "my-internal-pkg": false,
            }
        });
        let config = serde_json::from_value::<PythonBackendConfig>(json_data).unwrap();
        let map = config.pypi_conda_map.unwrap();
        assert_eq!(
            map.get("torch"),
            Some(&PypiCondaMapEntry::CondaName("pytorch".to_string()))
        );
        assert_eq!(map.get("my-internal-pkg"), Some(&PypiCondaMapEntry::Skip));
    }

    #[test]
    fn test_deserialize_pypi_conda_map_rejects_true() {
        let json_data = json!({
            "pypi-conda-map": {
                "torch": true,
            }
        });
        let err = serde_json::from_value::<PythonBackendConfig>(json_data).unwrap_err();
        assert!(err.to_string().contains("`true` is not supported"));
    }

    #[test]
    fn test_merge_pypi_conda_map_per_key() {
        let base = PythonBackendConfig {
            pypi_conda_map: Some(indexmap::indexmap! {
                "torch".to_string() => PypiCondaMapEntry::CondaName("pytorch".to_string()),
                "shared".to_string() => PypiCondaMapEntry::CondaName("base-name".to_string()),
            }),
            ..Default::default()
        };
        let target = PythonBackendConfig {
            pypi_conda_map: Some(indexmap::indexmap! {
                "shared".to_string() => PypiCondaMapEntry::Skip,
                "extra".to_string() => PypiCondaMapEntry::CondaName("extra-conda".to_string()),
            }),
            ..Default::default()
        };

        let merged = base.merge_with_target_config(&target).unwrap();
        let map = merged.pypi_conda_map.unwrap();
        // Base-only keys survive, target keys override or extend.
        assert_eq!(
            map.get("torch"),
            Some(&PypiCondaMapEntry::CondaName("pytorch".to_string()))
        );
        assert_eq!(map.get("shared"), Some(&PypiCondaMapEntry::Skip));
        assert_eq!(
            map.get("extra"),
            Some(&PypiCondaMapEntry::CondaName("extra-conda".to_string()))
        );

        // None + None stays None.
        let merged = PythonBackendConfig::default()
            .merge_with_target_config(&PythonBackendConfig::default())
            .unwrap();
        assert_eq!(merged.pypi_conda_map, None);
    }

    #[test]
    fn test_merge_with_target_config() {
        let mut base_env = indexmap::IndexMap::new();
        base_env.insert("BASE_VAR".to_string(), "base_value".to_string());
        base_env.insert("SHARED_VAR".to_string(), "base_shared".to_string());

        let base_config = PythonBackendConfig {
            noarch: Some(true),
            env: base_env,
            debug_dir: Some(PathBuf::from("/base/debug")),
            extra_args: vec!["-Cbuilddir=mybuilddir".into()],
            extra_input_globs: vec!["*.base".to_string()],
            compilers: Some(vec!["c".to_string()]),
            ignore_pyproject_manifest: Some(true),
            ignore_pypi_mapping: Some(true),
            pypi_conda_map: None,
            abi3: Some(true),
            skip_pyc_compilation: SkipPycCompilation::All(true),
        };

        let mut target_env = indexmap::IndexMap::new();
        target_env.insert("TARGET_VAR".to_string(), "target_value".to_string());
        target_env.insert("SHARED_VAR".to_string(), "target_shared".to_string());

        let target_config = PythonBackendConfig {
            noarch: Some(false),
            env: target_env,
            debug_dir: None,
            extra_args: vec![],
            extra_input_globs: vec!["*.target".to_string()],
            compilers: Some(vec!["cxx".to_string(), "rust".to_string()]),
            ignore_pyproject_manifest: Some(false),
            ignore_pypi_mapping: Some(false),
            pypi_conda_map: None,
            abi3: Some(false),
            skip_pyc_compilation: SkipPycCompilation::Globs(vec!["tests/**".to_string()]),
        };

        let merged = base_config
            .merge_with_target_config(&target_config)
            .unwrap();

        // noarch should use target value
        assert_eq!(merged.noarch, Some(false));

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
            Some(vec!["cxx".to_string(), "rust".to_string()])
        );
        // ignore_pyproject_manifest should use target value
        assert_eq!(merged.ignore_pyproject_manifest, Some(false));
        // ignore_pypi_mapping should use target value
        assert_eq!(merged.ignore_pypi_mapping, Some(false));
        // abi3 should use target value
        assert_eq!(merged.abi3, Some(false));
        // skip_pyc_compilation should use target value
        assert_eq!(
            merged.skip_pyc_compilation,
            SkipPycCompilation::Globs(vec!["tests/**".to_string()])
        );
    }

    #[test]
    fn test_merge_with_empty_target_config() {
        let mut base_env = indexmap::IndexMap::new();
        base_env.insert("BASE_VAR".to_string(), "base_value".to_string());

        let base_config = PythonBackendConfig {
            noarch: Some(true),
            env: base_env,
            debug_dir: Some(PathBuf::from("/base/debug")),
            extra_args: vec!["-Cbuilddir=mybuilddir".into()],
            extra_input_globs: vec!["*.base".to_string()],
            compilers: None,
            ignore_pyproject_manifest: Some(true),
            ignore_pypi_mapping: Some(true),
            pypi_conda_map: None,
            abi3: None,
            skip_pyc_compilation: SkipPycCompilation::All(true),
        };

        let empty_target_config = PythonBackendConfig::default();

        let merged = base_config
            .merge_with_target_config(&empty_target_config)
            .unwrap();

        // Should keep base values when target is empty
        assert_eq!(merged.noarch, Some(true));
        assert_eq!(merged.env.get("BASE_VAR"), Some(&"base_value".to_string()));
        assert_eq!(merged.debug_dir, Some(PathBuf::from("/base/debug")));
        assert_eq!(merged.extra_input_globs, vec!["*.base".to_string()]);
        assert_eq!(merged.compilers, None);
        assert_eq!(merged.ignore_pyproject_manifest, Some(true));
        assert_eq!(merged.ignore_pypi_mapping, Some(true));
    }

    #[test]
    fn test_merge_abi3_behavior() {
        // Target overrides base
        let base = PythonBackendConfig {
            abi3: Some(true),
            ..Default::default()
        };
        let target = PythonBackendConfig {
            abi3: Some(false),
            ..Default::default()
        };
        let merged = base.merge_with_target_config(&target).unwrap();
        assert_eq!(merged.abi3, Some(false));

        // Target None keeps base
        let target_none = PythonBackendConfig {
            abi3: None,
            ..Default::default()
        };
        let merged = base.merge_with_target_config(&target_none).unwrap();
        assert_eq!(merged.abi3, Some(true));

        // Both None stays None
        let base_none = PythonBackendConfig::default();
        let merged = base_none.merge_with_target_config(&target_none).unwrap();
        assert_eq!(merged.abi3, None);
    }

    #[test]
    fn test_deserialize_abi3_field() {
        let json_data = json!({"abi3": true});
        let config: PythonBackendConfig = serde_json::from_value(json_data).unwrap();
        assert_eq!(config.abi3, Some(true));

        let json_data = json!({"abi3": false});
        let config: PythonBackendConfig = serde_json::from_value(json_data).unwrap();
        assert_eq!(config.abi3, Some(false));

        // Not specified should be None
        let json_data = json!({});
        let config: PythonBackendConfig = serde_json::from_value(json_data).unwrap();
        assert_eq!(config.abi3, None);
    }

    #[test]
    fn test_merge_noarch_behavior() {
        let base_config = PythonBackendConfig {
            noarch: Some(true),
            ..Default::default()
        };

        let target_config = PythonBackendConfig {
            noarch: None,
            ..Default::default()
        };

        let merged = base_config
            .merge_with_target_config(&target_config)
            .unwrap();

        // When target has None, should keep base value
        assert_eq!(merged.noarch, Some(true));

        // Test the reverse
        let base_config = PythonBackendConfig {
            noarch: None,
            ..Default::default()
        };

        let target_config = PythonBackendConfig {
            noarch: Some(false),
            ..Default::default()
        };

        let merged = base_config
            .merge_with_target_config(&target_config)
            .unwrap();

        // When target has value, should use target value
        assert_eq!(merged.noarch, Some(false));

        // Test when both have values - target should override base
        let base_config = PythonBackendConfig {
            noarch: Some(true),
            ..Default::default()
        };

        let target_config = PythonBackendConfig {
            noarch: Some(false),
            ..Default::default()
        };

        let merged = base_config
            .merge_with_target_config(&target_config)
            .unwrap();

        // Target value should override base value
        assert_eq!(merged.noarch, Some(false));
    }
}
