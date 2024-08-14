use std::{
    env,
    fmt::Formatter,
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use manifest::Manifest;
use miette::IntoDiagnostic;
use rattler_repodata_gateway::Gateway;
use reqwest_middleware::ClientWithMiddleware;
use std::fmt::Debug;

mod document;
mod environment;
mod errors;
mod manifest;
mod parsed_manifest;

/// The pixi global project, this main struct to interact with the pixi global project.
/// This struct holds the `Manifest` and has functions to modify
/// or request information from it. This allows in the future to have multiple manifests
/// linked to a pixi global project.
#[derive(Clone)]
pub struct Project {
    /// Root folder of the project
    root: PathBuf,
    /// Reqwest client shared for this project.
    /// This is wrapped in a `OnceLock` to allow for lazy initialization.
    client: OnceLock<(reqwest::Client, ClientWithMiddleware)>,
    /// The repodata gateway to use for answering queries about repodata.
    /// This is wrapped in a `OnceLock` to allow for lazy initialization.
    repodata_gateway: OnceLock<Gateway>,
    /// The manifest for the project
    pub(crate) manifest: Manifest,
}

impl Debug for Project {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Global Project")
            .field("root", &self.root)
            .field("manifest", &self.manifest)
            .finish()
    }
}

impl Project {
    /// Constructs a new instance from an internal manifest representation
    fn from_manifest(manifest: Manifest) -> Self {
        let root = manifest
            .path
            .parent()
            .expect("manifest path should always have a parent")
            .to_owned();

        Self {
            root,
            client: Default::default(),
            repodata_gateway: Default::default(),
            manifest,
        }
    }

    /// Constructs a project from a manifest.
    pub fn from_str(manifest_path: &Path, content: &str) -> miette::Result<Self> {
        let manifest = Manifest::from_str(manifest_path, content)?;
        Ok(Self::from_manifest(manifest))
    }

    /// Discovers the project manifest file in path set by `PIXI_GLOBAL_MANIFESTS`
    /// or alternatively at `~/.pixi/manifests/pixi-global.toml`.
    /// If the manifest doesn't exist yet, and empty one will be created.
    pub fn discover() -> miette::Result<Self> {
        let manifest_dir = env::var("PIXI_GLOBAL_MANIFESTS")
            .map(PathBuf::from)
            .or_else(|_| Self::default_dir())?;

        fs::create_dir_all(&manifest_dir).into_diagnostic()?;

        let manifest_path = manifest_dir.join("pixi-global.toml");

        if !manifest_path.exists() {
            fs::File::create(&manifest_path).into_diagnostic()?;
        }
        Self::from_path(&manifest_path)
    }

    /// Get default dir for the pixi global manifest
    fn default_dir() -> miette::Result<PathBuf> {
        // If environment variable is not set, use default directory
        let default_dir = dirs::home_dir()
            .ok_or_else(|| miette::miette!("Could not get home directory"))?
            .join(".pixi/manifests");
        Ok(default_dir)
    }

    /// Loads a project from manifest file.
    pub fn from_path(manifest_path: &Path) -> miette::Result<Self> {
        let manifest = Manifest::from_path(manifest_path)?;
        Ok(Project::from_manifest(manifest))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use fake::{faker::filesystem::zh_tw::FilePath, Fake};

    const SIMPLE_MANIFEST: &str = r#"   
        [envs.python.dependencies]
        python = "3.11.*"
        [envs.python.exposed]
        python = "python"
        "#;

    #[test]

    fn test_project_from_str() {
        let manifest_path: PathBuf = FilePath().fake();

        let project = Project::from_str(&manifest_path, SIMPLE_MANIFEST).unwrap();
        assert_eq!(project.root, manifest_path.parent().unwrap());
    }

    #[test]
    fn test_project_from_path() {
        let tempdir = tempfile::tempdir().unwrap();
        let manifest_path = tempdir.path().join("pixi-global.toml");

        // Create and write to the pixi-global.toml file
        let mut file = fs::File::create(&manifest_path).unwrap();
        file.write_all(SIMPLE_MANIFEST.as_bytes()).unwrap();
        let project = Project::from_path(&manifest_path).unwrap();
        assert_eq!(project.root, manifest_path.parent().unwrap());
    }

    #[test]
    fn test_project_discover() {
        let tempdir = tempfile::tempdir().unwrap();
        let manifest_dir = tempdir.path();
        env::set_var("PIXI_GLOBAL_MANIFESTS", manifest_dir);
        let project = Project::discover().unwrap();
        assert!(project.manifest.path.exists());
        assert_eq!(project.manifest.path, manifest_dir.join("pixi-global.toml"))
    }

    #[test]
    fn test_project_from_manifest() {
        let manifest_path: PathBuf = FilePath().fake();

        let manifest = Manifest::from_str(&manifest_path, SIMPLE_MANIFEST).unwrap();
        let project = Project::from_manifest(manifest);
        assert_eq!(project.root, manifest_path.parent().unwrap());
    }

    #[test]
    fn test_project_default_dir() {
        Project::default_dir().unwrap();
    }
}
