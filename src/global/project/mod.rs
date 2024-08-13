use std::{
    borrow::Borrow,
    fmt::Formatter,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use manifest::Manifest;
use miette::NamedSource;
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

    /// Discovers the project manifest file in path set by `PIXI_GLOBAL_MANIFEST`
    /// or alternatively at `~/.pixi/`
    pub fn discover() -> miette::Result<Self> {
        let project_toml = find_project_manifest();

        Self::from_path(&project_toml)
    }

    /// Loads a project from manifest file.
    pub fn from_path(manifest_path: &Path) -> miette::Result<Self> {
        let manifest = Manifest::from_path(manifest_path)?;
        Ok(Project::from_manifest(manifest))
    }
}
