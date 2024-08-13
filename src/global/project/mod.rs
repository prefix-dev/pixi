use std::{
    borrow::Borrow,
    env,
    fmt::Formatter,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use manifest::Manifest;
use rattler_repodata_gateway::Gateway;
use reqwest_middleware::ClientWithMiddleware;
use std::fmt::Debug;

mod manifest;

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
        f.debug_struct("Project")
            .field("root", &self.root)
            .field("manifest", &self.manifest)
            .finish()
    }
}

impl Borrow<ParsedManifest> for Project {
    fn borrow(&self) -> &ParsedManifest {
        self.manifest.borrow()
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
    /// or alternatively at `~/.pixi/manifests/pixi-global.toml`
    pub fn discover() -> miette::Result<Self> {
        // Retrieve the path from the environment variable
        let manifest_path = env::var("PIXI_GLOBAL_MANIFESTS")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                // Default directory if the environment variable is not set
                let mut default_dir = dirs::home_dir()?.join(".pixi/manifests");
            })
            .join("pixi-global.toml");

        if manifest_path.exists() {
            Self::from_path(&manifest_path)
        } else {
            miette::bail!("Manifest file not found at {}", manifest_path.display())
        }
    }

    /// Loads a project from manifest file.
    pub fn from_path(manifest_path: &Path) -> miette::Result<Self> {
        let manifest = Manifest::from_path(manifest_path)?;
        Ok(Project::from_manifest(manifest))
    }
}
