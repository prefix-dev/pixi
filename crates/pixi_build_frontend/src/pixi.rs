use std::path::{Path, PathBuf};

use crate::tool::{IsolatedToolSpec, Tool, ToolSpec};
use crate::Metadata;
use pixi_consts::consts;
use pixi_manifest::Manifest;
use rattler_conda_types::{MatchSpec, ParseStrictness::Strict};

/// A protocol that uses a pixi manifest to invoke a build backend .
#[derive(Debug)]
pub(crate) struct PixiProtocol {
    _manifest: Manifest,
    backend_spec: ToolSpec,
}

impl PixiProtocol {
    /// Constructs a new instance from a manifest.
    pub fn new(manifest: Manifest) -> Self {
        // TODO: Replace this with something that we read from the manifest.
        let backend_spec =
            IsolatedToolSpec::from_specs(vec![
                MatchSpec::from_str("pixi-build-python", Strict).unwrap()
            ])
            .into();

        Self {
            _manifest: manifest,
            backend_spec,
        }
    }

    /// Discovers a pixi project in the given source directory.
    pub fn discover(source_dir: &Path) -> miette::Result<Option<Self>> {
        if let Some(manifest_path) = find_pixi_manifest(source_dir) {
            let manifest = Manifest::from_path(&manifest_path)?;
            return Ok(Some(Self::new(manifest)));
        }
        Ok(None)
    }

    /// Returns the backend spec of this protocol.
    pub fn backend_tool(&self) -> ToolSpec {
        self.backend_spec.clone()
    }

    /// Extract metadata from the recipe.
    pub fn get_metadata(&self, _backend: &Tool) -> miette::Result<Metadata> {
        todo!("extract metadata from pixi manifest")
    }
}

/// Try to find a pixi manifest in the given source directory.
fn find_pixi_manifest(source_dir: &Path) -> Option<PathBuf> {
    let pixi_manifest_path = source_dir.join(consts::PROJECT_MANIFEST);
    if pixi_manifest_path.exists() {
        return Some(pixi_manifest_path);
    }

    let pyproject_manifest_path = source_dir.join(consts::PYPROJECT_MANIFEST);
    // TODO: Really check if this is a pixi project.
    if pyproject_manifest_path.is_file() {
        return Some(pyproject_manifest_path);
    }

    None
}
