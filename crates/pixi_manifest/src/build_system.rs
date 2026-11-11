//! Defines the build section for the pixi manifest.

use std::hash::{Hash, Hasher};

use indexmap::IndexMap;
use pixi_spec::{PixiSpec, SourceLocationSpec};
use rattler_conda_types::{Flag, NamedChannelOrUrl};

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

    /// Optional package source location
    pub source: Option<SourceLocationSpec>,

    /// Additional configuration for the build backend.
    pub config: Option<serde_value::Value>,

    /// V3 package variant flags declared by the source package.
    pub flags: Vec<Flag>,

    /// Target-specific configuration for different platforms
    pub target_config: Option<IndexMap<TargetSelector, serde_value::Value>>,

    /// An optional prefix to prepend to the auto-generated build string.
    pub build_string_prefix: Option<String>,

    /// The build number configured by the user.
    pub build_number: Option<u64>,

    /// Names of environment variables to expose as secrets to the build
    /// script. Values are looked up at build time from the host environment by
    /// the build backend; only the names live in the manifest. Stored as a
    /// set since order is not observable.
    pub secrets: std::collections::BTreeSet<String>,
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
            flags: Vec::new(),
            target_config: None,
            build_string_prefix: None,
            build_number: None,
            secrets: std::collections::BTreeSet::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Hash)]
pub struct BuildBackend {
    /// The name of the build backend to install
    pub name: rattler_conda_types::PackageName,

    /// The spec for the backend
    pub spec: PixiSpec,
}

impl Hash for PackageBuild {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Bind every field so adding a new one fails to compile until it is
        // accounted for here. A silently unhashed field would let two builds
        // that differ only in that field collide in the content-addressed cache.
        let Self {
            backend,
            additional_dependencies,
            channels,
            source,
            config,
            flags,
            target_config,
            build_string_prefix,
            build_number,
            secrets,
        } = self;
        backend.hash(state);
        // The dependency and target-config maps are `IndexMap`s; their
        // declaration order is stable, so hash their entries in order.
        additional_dependencies.len().hash(state);
        for (name, spec) in additional_dependencies {
            name.hash(state);
            spec.hash(state);
        }
        channels.hash(state);
        source.hash(state);
        config.hash(state);
        flags.hash(state);
        match target_config {
            Some(target_config) => {
                target_config.len().hash(state);
                for (selector, config) in target_config {
                    selector.hash(state);
                    config.hash(state);
                }
            }
            None => usize::MAX.hash(state),
        }
        build_string_prefix.hash(state);
        build_number.hash(state);
        secrets.hash(state);
    }
}

impl PackageBuild {
    /// Parses a build system in isolation. Rejects `workspace = true` on the
    /// backend since there is no workspace context to resolve against.
    pub fn from_toml_str(source: &str) -> Result<WithWarnings<Self>, TomlError> {
        TomlPackageBuild::from_toml_str(source).and_then(|build| {
            if let crate::toml::BackendSpec::Inherited { marker_span, .. } =
                &build.backend.value.spec
            {
                return Err(crate::error::GenericError::new(
                    "`workspace = true` on `[build.backend]` requires a workspace context",
                )
                .with_span(marker_span.clone())
                .into());
            }
            build.into_build_system(&indexmap::IndexMap::new())
        })
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
            source = { git = "https://github.com/conda-forge/numpy-feedstock", rev ="ee87916a49d5e96d4f322f68c3650e8ff6b8866b" }
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
    fn deserialize_build_with_git_source_branch() {
        let toml = r#"
            backend = { name = "pixi-build-rattler-build", version = "0.1.*" }
            channels = [
              "https://prefix.dev/pixi-build-backends",
              "https://prefix.dev/conda-forge",
            ]
            source = { git = "https://github.com/conda-forge/numpy-feedstock", branch = "main" }
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
    fn deserialize_build_with_git_source_tag() {
        let toml = r#"
            backend = { name = "pixi-build-rattler-build", version = "0.1.*" }
            channels = [
              "https://prefix.dev/pixi-build-backends",
              "https://prefix.dev/conda-forge",
            ]
            source = { git = "https://github.com/conda-forge/numpy-feedstock", tag = "v1.0.0" }
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
                source = {{ path = "{path}" }}
                "#
            );

            let build = PackageBuild::from_toml_str(&toml).unwrap();
            assert!(build.value.source.is_some(), "Failed for path: {path}");

            if let Some(source) = &build.value.source {
                match &source {
                    pixi_spec::SourceLocationSpec::Path(path_spec) => {
                        // Verify the path is preserved correctly
                        assert_eq!(
                            path_spec.path.as_str(),
                            path,
                            "Path mismatch for input: {path}"
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
                    _ => panic!("Expected a path source spec for: {path}"),
                }
            }
        }
    }
}
