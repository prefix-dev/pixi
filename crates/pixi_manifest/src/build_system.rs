//! Defines the build section for the pixi manifest.

use indexmap::IndexMap;
use pixi_spec::{PixiSpec, SourceLocationSpec};
use rattler_conda_types::NamedChannelOrUrl;

use crate::TargetSelector;
use crate::{
    TomlError, WithWarnings,
    toml::{FromTomlStr, TomlPackageBuild},
};

/// A build section in the pixi manifest.
/// that defines what backend is used to build the project.
#[derive(Debug, Clone, PartialEq)]
pub struct PackageBuild {
    /// Information about the build backend
    pub backend: BuildBackend,

    /// Additional dependencies that should be installed alongside the backend.
    pub additional_dependencies: IndexMap<rattler_conda_types::PackageName, PixiSpec>,

    /// The channels to use for fetching build tools. If this is `None` the
    /// channels from the containing workspace should be used.
    pub channels: Option<Vec<NamedChannelOrUrl>>,

    /// Optional package source specification
    pub source: Option<SourceLocationSpec>,

    /// Additional configuration for the build backend.
    pub config: Option<serde_value::Value>,

    /// Target-specific configuration for different platforms
    pub target_config: Option<IndexMap<TargetSelector, serde_value::Value>>,
}

impl PackageBuild {
    /// Constructs a new instance from just a backend and channels.
    pub fn new(backend: BuildBackend, channels: Vec<NamedChannelOrUrl>) -> Self {
        Self {
            backend,
            channels: Some(channels),
            additional_dependencies: IndexMap::default(),
            source: None,
            config: None,
            target_config: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuildBackend {
    /// The name of the build backend to install
    pub name: rattler_conda_types::PackageName,

    /// The spec for the backend
    pub spec: PixiSpec,
}

impl PackageBuild {
    /// Parses the specified string as a toml representation of a build system.
    pub fn from_toml_str(source: &str) -> Result<WithWarnings<Self>, TomlError> {
        TomlPackageBuild::from_toml_str(source).and_then(TomlPackageBuild::into_build_system)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_build() {
        let toml = r#"
            backend = { name = "pixi-build-python", version = "12.*" }
            "#;

        let build = PackageBuild::from_toml_str(toml).unwrap();
        assert_eq!(build.value.backend.name.as_source(), "pixi-build-python");
    }

    #[test]
    fn deserialize_build_with_path_source() {
        let toml = r#"
            backend = { name = "pixi-build-rattler-build", version = "0.1.*" }
            channels = [
              "https://prefix.dev/pixi-build-backends",
              "https://prefix.dev/conda-forge",
            ]
            source = { path = "/path/to/source" }
            "#;

        let build = PackageBuild::from_toml_str(toml).unwrap();
        assert_eq!(
            build.value.backend.name.as_source(),
            "pixi-build-rattler-build"
        );
        assert!(build.value.source.is_some());
        assert!(!build.value.source.unwrap().is_git());
    }

    #[test]
    fn deserialize_build_with_git_source() {
        let toml = r#"
            backend = { name = "pixi-build-rattler-build", version = "0.1.*" }
            channels = [
              "https://prefix.dev/pixi-build-backends",
              "https://prefix.dev/conda-forge",
            ]
            source = { git = "https://github.com/conda-forge/numpy-feedstock" }
            "#;

        let build = PackageBuild::from_toml_str(toml).unwrap();
        assert_eq!(
            build.value.backend.name.as_source(),
            "pixi-build-rattler-build"
        );
        assert!(build.value.source.is_some());
        assert!(build.value.source.unwrap().is_git());
    }

    #[test]
    fn deserialize_build_with_url_source() {
        let toml = r#"
            backend = { name = "pixi-build-rattler-build", version = "0.1.*" }
            channels = [
              "https://prefix.dev/pixi-build-backends",
              "https://prefix.dev/conda-forge",
            ]
            source = { url = "https://github.com/conda-forge/numpy-feedstock/archive/main.zip" }
            "#;

        let build = PackageBuild::from_toml_str(toml).unwrap();
        assert_eq!(
            build.value.backend.name.as_source(),
            "pixi-build-rattler-build"
        );
        assert!(build.value.source.is_some());
        assert!(!build.value.source.as_ref().unwrap().is_git());
    }

    #[test]
    fn deserialize_build_with_relative_path_source() {
        let toml = r#"
            backend = { name = "pixi-build-rattler-build", version = "0.1.*" }
            channels = [
              "https://prefix.dev/pixi-build-backends",
              "https://prefix.dev/conda-forge",
            ]
            source = { path = "../other-source" }
            "#;

        let build = PackageBuild::from_toml_str(toml).unwrap();
        assert_eq!(
            build.value.backend.name.as_source(),
            "pixi-build-rattler-build"
        );
        assert!(build.value.source.is_some());

        // Verify it's a path source and contains the relative path
        if let Some(source) = &build.value.source {
            match &source {
                pixi_spec::SourceLocationSpec::Path(path_spec) => {
                    assert_eq!(path_spec.path.as_str(), "../other-source");
                }
                _ => panic!("Expected a path source spec"),
            }
        }
    }

    #[test]
    fn deserialize_build_with_home_directory_path_source() {
        let toml = r#"
            backend = { name = "pixi-build-rattler-build", version = "0.1.*" }
            channels = [
              "https://prefix.dev/pixi-build-backends",
              "https://prefix.dev/conda-forge",
            ]
            source = { path = "~/my-source" }
            "#;

        let build = PackageBuild::from_toml_str(toml).unwrap();
        assert_eq!(
            build.value.backend.name.as_source(),
            "pixi-build-rattler-build"
        );
        assert!(build.value.source.is_some());

        // Verify it's a path source and contains the home directory path
        if let Some(source) = &build.value.source {
            match &source {
                pixi_spec::SourceLocationSpec::Path(path_spec) => {
                    assert_eq!(path_spec.path.as_str(), "~/my-source");
                }
                _ => panic!("Expected a path source spec"),
            }
        }
    }

    #[test]
    fn test_path_resolution_relative_paths() {
        use tempfile::TempDir;

        // Create a temporary directory structure for testing
        let temp_dir = TempDir::new().unwrap();
        let workspace_root = temp_dir.path();

        let toml = r#"
            backend = { name = "pixi-build-rattler-build", version = "0.1.*" }
            source = { path = "../other-source" }
            "#;

        let build = PackageBuild::from_toml_str(toml).unwrap();
        assert!(build.value.source.is_some());

        if let Some(source) = &build.value.source {
            match &source {
                pixi_spec::SourceLocationSpec::Path(path_spec) => {
                    // Test that the path spec can resolve relative paths correctly
                    let resolved = path_spec.resolve(workspace_root).unwrap();

                    // The resolved path should contain "other-source" (canonicalized may differ, but should point to same logical location)
                    assert!(resolved.to_string_lossy().contains("other-source"));

                    // Test that relative path is preserved in the spec itself
                    assert_eq!(path_spec.path.as_str(), "../other-source");
                }
                _ => panic!("Expected a path source spec"),
            }
        }
    }

    #[test]
    fn test_path_resolution_absolute_paths() {
        use std::path::Path;

        let toml = r#"
            backend = { name = "pixi-build-rattler-build", version = "0.1.*" }
            source = { path = "/absolute/path/to/source" }
            "#;

        let build = PackageBuild::from_toml_str(toml).unwrap();
        assert!(build.value.source.is_some());

        if let Some(source) = &build.value.source {
            match &source {
                pixi_spec::SourceLocationSpec::Path(path_spec) => {
                    // Test that absolute paths are returned as-is during resolution
                    let resolved = path_spec.resolve("/workspace/root").unwrap();
                    assert_eq!(resolved, Path::new("/absolute/path/to/source"));

                    // Test that absolute path is preserved in the spec itself
                    assert_eq!(path_spec.path.as_str(), "/absolute/path/to/source");
                }
                _ => panic!("Expected a path source spec"),
            }
        }
    }

    #[test]
    fn test_various_relative_path_formats() {
        let test_cases = vec![
            "./current-dir",
            "../parent-dir",
            "../../grandparent",
            "subdir/nested",
            "../sibling/nested",
        ];

        for path in test_cases {
            let toml = format!(
                r#"
                backend = {{ name = "pixi-build-rattler-build", version = "0.1.*" }}
                source = {{ path = "{}" }}
                "#,
                path
            );

            let build = PackageBuild::from_toml_str(&toml).unwrap();
            assert!(build.value.source.is_some(), "Failed for path: {}", path);

            if let Some(source) = &build.value.source {
                match &source {
                    pixi_spec::SourceLocationSpec::Path(path_spec) => {
                        // Verify the path is preserved correctly
                        assert_eq!(
                            path_spec.path.as_str(),
                            path,
                            "Path mismatch for input: {}",
                            path
                        );

                        // Verify it can be resolved (using a dummy workspace root)
                        // Use a cross-platform workspace root
                        let workspace_root = if cfg!(windows) {
                            std::path::Path::new("C:\\workspace")
                        } else {
                            std::path::Path::new("/workspace")
                        };
                        let resolved = path_spec.resolve(workspace_root).unwrap();

                        // The resolved path should be different from the input (unless it was absolute)
                        assert!(
                            resolved.is_absolute(),
                            "Resolved path should be absolute for: {}, but got {}",
                            path,
                            resolved.display()
                        );
                    }
                    _ => panic!("Expected a path source spec for: {}", path),
                }
            }
        }
    }
}
