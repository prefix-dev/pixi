use std::path::{Path, PathBuf};

/// Handles the project's manifest file.
/// This struct is responsible for reading, parsing, editing, and saving the
/// manifest. It encapsulates all logic related to the manifest's TOML format
/// and structure. The manifest data is represented as a [`ParsedManifest`]
/// struct for easy manipulation.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// The path to the manifest file
    pub path: PathBuf,

    /// The raw contents of the manifest file
    pub contents: String,

    /// Editable toml document
    pub document: ManifestSource,

    /// The parsed manifest
    pub parsed: ParsedManifest,
}

impl Manifest {
    /// Create a new manifest from a path
    pub fn from_path(path: impl AsRef<Path>) -> miette::Result<Self> {
        let manifest_path = dunce::canonicalize(path.as_ref()).into_diagnostic()?;
        let contents = std::fs::read_to_string(path.as_ref()).into_diagnostic()?;
        Self::from_str(manifest_path.as_ref(), contents)
    }

    /// Create a new manifest from a string
    pub fn from_str(manifest_path: &Path, contents: impl Into<String>) -> miette::Result<Self> {
        todo!()
    }

    /// Adds an environment to the project.
    pub fn add_environment(&mut self, name: String) -> miette::Result<()> {
        todo!()
    }

    /// Removes an environment from the project.
    pub fn remove_environment(&mut self, name: &str) -> miette::Result<bool> {
        todo!()
    }

    /// Add a matchspec to the manifest
    pub fn add_dependency(
        &mut self,
        spec: &MatchSpec,
        spec_type: SpecType,
        platforms: &[Platform],
        feature_name: &FeatureName,
        overwrite_behavior: DependencyOverwriteBehavior,
        channel_config: &ChannelConfig,
    ) -> miette::Result<bool> {
        todo!()
    }

    /// Removes a dependency based on `SpecType`.
    pub fn remove_dependency(
        &mut self,
        dep: &PackageName,
        spec_type: SpecType,
        platforms: &[Platform],
        feature_name: &FeatureName,
    ) -> miette::Result<()> {
        todo!()
    }
}
