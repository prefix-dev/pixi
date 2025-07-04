//! Defines the build section for the pixi manifest.

use indexmap::IndexMap;
use pixi_spec::BinarySpec;
use rattler_conda_types::NamedChannelOrUrl;

use crate::toml::FromTomlStr;
use crate::{Targets, TomlError, toml::TomlPackageBuild};

/// A build section in the pixi manifest.
/// that defines what backend is used to build the project.
#[derive(Debug, Clone)]
pub struct PackageBuild {
    /// Information about the build backend
    pub backend: BuildBackend,

    /// Additional dependencies that should be installed alongside the backend.
    pub additional_dependencies: IndexMap<rattler_conda_types::PackageName, BinarySpec>,

    /// The channels to use for fetching build tools. If this is `None` the
    /// channels from the containing workspace should be used.
    pub channels: Option<Vec<NamedChannelOrUrl>>,

    /// Additional configuration for the build backend.
    pub configuration: Option<serde_value::Value>,

    /// Target-specific configuration for different platforms
    pub targets: Targets<BuildTarget>,
}

/// Represents target-specific build configuration
#[derive(Debug, Clone, Default)]
pub struct BuildTarget {
    /// Target-specific configuration for the build backend.
    pub configuration: Option<serde_value::Value>,
}

impl PackageBuild {
    /// Constructs a new instance from just a backend and channels.
    pub fn new(backend: BuildBackend, channels: Vec<NamedChannelOrUrl>) -> Self {
        Self {
            backend,
            channels: Some(channels),
            additional_dependencies: IndexMap::default(),
            configuration: None,
            targets: Targets::from_default_and_user_defined(
                BuildTarget::default(),
                IndexMap::default(),
            ),
        }
    }

    /// Returns the resolved configuration for the given platform.
    ///
    /// This method merges the default configuration with platform-specific configuration,
    /// with platform-specific configuration taking precedence.
    pub fn configuration(
        &self,
        platform: Option<rattler_conda_types::Platform>,
    ) -> Option<serde_value::Value> {
        use serde_value::Value;

        let mut merged_config: Option<Value> = None;

        // Start with the default configuration if it exists
        if let Some(default_config) = &self.configuration {
            merged_config = Some(default_config.clone());
        }

        // Collect all configurations that apply to this platform and merge them
        for target in self.targets.resolve(platform) {
            if let Some(config) = &target.configuration {
                match &merged_config {
                    None => {
                        merged_config = Some(config.clone());
                    }
                    Some(existing) => {
                        // Merge the configurations - target-specific config takes precedence
                        merged_config = Some(merge_serde_values(existing, config));
                    }
                }
            }
        }

        merged_config
    }
}

/// Merges two serde_value::Value objects, with `override_value` taking precedence
fn merge_serde_values(
    base: &serde_value::Value,
    override_value: &serde_value::Value,
) -> serde_value::Value {
    use serde_value::Value;

    match (base, override_value) {
        (Value::Map(base_map), Value::Map(override_map)) => {
            let mut merged = base_map.clone();
            for (key, value) in override_map {
                if let Some(existing_value) = merged.get(key) {
                    // Recursively merge nested objects
                    merged.insert(key.clone(), merge_serde_values(existing_value, value));
                } else {
                    merged.insert(key.clone(), value.clone());
                }
            }
            Value::Map(merged)
        }
        // For non-map values, override takes precedence
        _ => override_value.clone(),
    }
}

#[derive(Debug, Clone)]
pub struct BuildBackend {
    /// The name of the build backend to install
    pub name: rattler_conda_types::PackageName,

    /// The spec for the backend
    pub spec: BinarySpec,
}

impl PackageBuild {
    /// Parses the specified string as a toml representation of a build system.
    pub fn from_toml_str(source: &str) -> Result<Self, TomlError> {
        TomlPackageBuild::from_toml_str(source).and_then(TomlPackageBuild::into_build_system)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rattler_conda_types::Platform;

    #[test]
    fn deserialize_build() {
        let toml = r#"
            backend = { name = "pixi-build-python", version = "12.*" }
            "#;

        let build = PackageBuild::from_toml_str(toml).unwrap();
        assert_eq!(build.backend.name.as_source(), "pixi-build-python");
    }

    #[test]
    fn test_configuration_no_targets() {
        let build = PackageBuild::new(
            BuildBackend {
                name: "test-backend".parse().unwrap(),
                spec: pixi_spec::BinarySpec::any(),
            },
            vec![],
        );

        assert_eq!(build.configuration(None), None);
        assert_eq!(build.configuration(Some(Platform::Linux64)), None);
    }

    #[test]
    fn test_configuration_with_default_only() {
        let toml = r#"
            backend = { name = "pixi-build-python", version = "*" }
            configuration = { noarch = true, env = { SETUPTOOLS_SCM_PRETEND_VERSION = "1.0.0" } }
            "#;

        let build = PackageBuild::from_toml_str(toml).unwrap();

        // Should get the default configuration for any platform
        let config = build.configuration(Some(Platform::Linux64)).unwrap();
        let json_config: serde_json::Value = config.deserialize_into().unwrap();
        assert!(json_config["noarch"].as_bool().unwrap());
        assert_eq!(
            json_config["env"]["SETUPTOOLS_SCM_PRETEND_VERSION"]
                .as_str()
                .unwrap(),
            "1.0.0"
        );

        // Should also work with no platform specified
        let config = build.configuration(None).unwrap();
        let json_config: serde_json::Value = config.deserialize_into().unwrap();
        assert!(json_config["noarch"].as_bool().unwrap());
        assert_eq!(
            json_config["env"]["SETUPTOOLS_SCM_PRETEND_VERSION"]
                .as_str()
                .unwrap(),
            "1.0.0"
        );
    }

    #[test]
    fn test_configuration_with_target_specific() {
        let toml = r#"
backend = { name = "pixi-build-python", version = "*" }

[configuration]
noarch = true
extra-input-globs = ["data/**/*", "*.md"]

[configuration.env]
SETUPTOOLS_SCM_PRETEND_VERSION = "1.0.0"

[target.linux-64.configuration]
noarch = false
debug-dir = ".build-debug-linux"

[target.linux-64.configuration.env]
CFLAGS = "-O3"
CC = "gcc-12"

[target.win-64.configuration]
extra-input-globs = ["data/**/*", "*.md", "windows/**/*"]

[target.win-64.configuration.env]
MSVC_VERSION = "2022"
            "#;

        let build = PackageBuild::from_toml_str(toml).unwrap();

        // Test Linux configuration (should merge default + linux-specific)
        let linux_config = build.configuration(Some(Platform::Linux64)).unwrap();
        let json_linux: serde_json::Value = linux_config.deserialize_into().unwrap();
        assert!(!json_linux["noarch"].as_bool().unwrap()); // overridden
        assert_eq!(
            json_linux["debug-dir"].as_str().unwrap(),
            ".build-debug-linux"
        ); // linux-specific

        // Test environment variable merging
        assert_eq!(
            json_linux["env"]["SETUPTOOLS_SCM_PRETEND_VERSION"]
                .as_str()
                .unwrap(),
            "1.0.0"
        ); // from default
        assert_eq!(json_linux["env"]["CFLAGS"].as_str().unwrap(), "-O3"); // linux-specific
        assert_eq!(json_linux["env"]["CC"].as_str().unwrap(), "gcc-12"); // linux-specific

        // Test Windows configuration (should merge default + windows-specific)
        let windows_config = build.configuration(Some(Platform::Win64)).unwrap();
        let json_windows: serde_json::Value = windows_config.deserialize_into().unwrap();
        assert!(json_windows["noarch"].as_bool().unwrap()); // from default
        assert_eq!(
            json_windows["env"]["SETUPTOOLS_SCM_PRETEND_VERSION"]
                .as_str()
                .unwrap(),
            "1.0.0"
        ); // from default
        assert_eq!(
            json_windows["env"]["MSVC_VERSION"].as_str().unwrap(),
            "2022"
        ); // windows-specific

        // Test array override
        let windows_globs = json_windows["extra-input-globs"].as_array().unwrap();
        assert_eq!(windows_globs.len(), 3); // windows override
        assert!(
            windows_globs
                .iter()
                .any(|g| g.as_str().unwrap() == "windows/**/*")
        );

        // Test macOS configuration (should only get default)
        let macos_config = build.configuration(Some(Platform::OsxArm64)).unwrap();
        let json_macos: serde_json::Value = macos_config.deserialize_into().unwrap();
        assert!(json_macos["noarch"].as_bool().unwrap());
        assert_eq!(
            json_macos["env"]["SETUPTOOLS_SCM_PRETEND_VERSION"]
                .as_str()
                .unwrap(),
            "1.0.0"
        );
        assert!(json_macos.get("debug-dir").is_none());
        assert!(json_macos["env"].get("CFLAGS").is_none());
        assert!(json_macos["env"].get("MSVC_VERSION").is_none());
    }

    #[test]
    fn test_configuration_target_only() {
        let toml = r#"
            backend = { name = "pixi-build-python", version = "*" }

            [target.linux-64.configuration]
            noarch = false
            debug-dir = ".build-debug"
            env = { CFLAGS = "-O3" }
            "#;

        let build = PackageBuild::from_toml_str(toml).unwrap();

        // Test Linux configuration (should only get linux-specific)
        let linux_config = build.configuration(Some(Platform::Linux64)).unwrap();
        let json_linux: serde_json::Value = linux_config.deserialize_into().unwrap();
        assert!(!json_linux["noarch"].as_bool().unwrap());
        assert_eq!(json_linux["debug-dir"].as_str().unwrap(), ".build-debug");
        assert_eq!(json_linux["env"]["CFLAGS"].as_str().unwrap(), "-O3");

        // Test other platforms (should get None)
        assert_eq!(build.configuration(Some(Platform::Win64)), None);
        assert_eq!(build.configuration(Some(Platform::OsxArm64)), None);
    }

    #[test]
    fn test_configuration_nested_merge() {
        let toml = r#"
            backend = { name = "pixi-build-python", version = "*" }

            [configuration.env]
            SETUPTOOLS_SCM_PRETEND_VERSION = "1.0.0"
            PYTHON_VERSION = "3.11"

            [configuration]
            noarch = true
            extra-input-globs = ["*.py", "*.md"]

            [target.linux-64.configuration.env]
            PYTHON_VERSION = "3.12"
            CFLAGS = "-O3 -march=native"

            [target.linux-64.configuration]
            noarch = false
            debug-dir = ".build-debug"
            "#;

        let build = PackageBuild::from_toml_str(toml).unwrap();

        let linux_config = build.configuration(Some(Platform::Linux64)).unwrap();
        let json_linux: serde_json::Value = linux_config.deserialize_into().unwrap();

        // Check env section is properly merged
        let env = &json_linux["env"];
        assert_eq!(
            env["SETUPTOOLS_SCM_PRETEND_VERSION"].as_str().unwrap(),
            "1.0.0"
        ); // from default
        assert_eq!(env["PYTHON_VERSION"].as_str().unwrap(), "3.12"); // overridden
        assert_eq!(env["CFLAGS"].as_str().unwrap(), "-O3 -march=native"); // linux-specific

        // Check other sections are properly merged
        assert!(!json_linux["noarch"].as_bool().unwrap()); // overridden
        assert_eq!(json_linux["debug-dir"].as_str().unwrap(), ".build-debug"); // linux-specific

        // Check array is preserved from default
        let globs = json_linux["extra-input-globs"].as_array().unwrap();
        assert_eq!(globs.len(), 2);
        assert!(globs.iter().any(|g| g.as_str().unwrap() == "*.py"));
        assert!(globs.iter().any(|g| g.as_str().unwrap() == "*.md"));
    }

    #[test]
    fn test_merge_serde_values() {
        use serde_json::json;

        let base = serde_value::to_value(json!({
            "a": 1,
            "b": {
                "x": "base",
                "y": 2
            },
            "c": [1, 2]
        }))
        .unwrap();

        let override_val = serde_value::to_value(json!({
            "b": {
                "x": "override",
                "z": 3
            },
            "c": [3, 4, 5],
            "d": "new"
        }))
        .unwrap();

        let merged = merge_serde_values(&base, &override_val);
        let result: serde_json::Value = merged.deserialize_into().unwrap();

        assert_eq!(result["a"], 1); // preserved from base
        assert_eq!(result["b"]["x"], "override"); // overridden
        assert_eq!(result["b"]["y"], 2); // preserved from base
        assert_eq!(result["b"]["z"], 3); // added from override
        assert_eq!(result["c"], json!([3, 4, 5])); // completely replaced
        assert_eq!(result["d"], "new"); // added from override
    }

    #[test]
    fn test_deserialize_with_targets() {
        let toml = r#"
backend = { name = "pixi-build-python", version = "*" }

[configuration]
noarch = true

[configuration.env]
SETUPTOOLS_SCM_PRETEND_VERSION = "1.0.0"

[target.linux-64.configuration]
noarch = false
debug-dir = ".build-debug-linux"

[target.win-64.configuration.env]
MSVC_VERSION = "2022"
            "#;

        let build = PackageBuild::from_toml_str(toml).unwrap();

        // Verify the targets were properly parsed
        assert!(build.targets.iter().count() > 0);

        // Test that configuration resolution works correctly
        let linux_config = build.configuration(Some(Platform::Linux64)).unwrap();
        let json_linux: serde_json::Value = linux_config.deserialize_into().unwrap();
        assert!(!json_linux["noarch"].as_bool().unwrap());
        assert_eq!(
            json_linux["debug-dir"].as_str().unwrap(),
            ".build-debug-linux"
        );
        assert_eq!(
            json_linux["env"]["SETUPTOOLS_SCM_PRETEND_VERSION"]
                .as_str()
                .unwrap(),
            "1.0.0"
        );

        let windows_config = build.configuration(Some(Platform::Win64)).unwrap();
        let json_windows: serde_json::Value = windows_config.deserialize_into().unwrap();
        assert!(json_windows["noarch"].as_bool().unwrap());
        assert_eq!(
            json_windows["env"]["SETUPTOOLS_SCM_PRETEND_VERSION"]
                .as_str()
                .unwrap(),
            "1.0.0"
        );
        assert_eq!(
            json_windows["env"]["MSVC_VERSION"].as_str().unwrap(),
            "2022"
        );
        assert!(json_windows.get("debug-dir").is_none());
    }
}
