mod protocol;

use std::path::{Path, PathBuf};

use pixi_consts::consts;
use pixi_manifest::Manifest;
pub use protocol::{InitializeError, Protocol};
use rattler_conda_types::{ChannelConfig, MatchSpec, ParseStrictness::Strict};

use crate::tool::{IsolatedToolSpec, Tool, ToolSpec};

/// A protocol that uses a pixi manifest to invoke a build backend .
#[derive(Debug)]
pub(crate) struct ProtocolBuilder {
    source_dir: PathBuf,
    _manifest: Manifest,
    backend_spec: ToolSpec,
    channel_config: ChannelConfig,
}

impl ProtocolBuilder {
    /// Constructs a new instance from a manifest.
    pub fn new(source_dir: PathBuf, manifest: Manifest) -> Self {
        // TODO: Replace this with something that we read from the manifest.
        let backend_spec =
            IsolatedToolSpec::from_specs(vec![
                MatchSpec::from_str("pixi-build-python", Strict).unwrap()
            ])
            .into();

        Self {
            source_dir,
            _manifest: manifest,
            backend_spec,
            channel_config: ChannelConfig::default_with_root_dir(PathBuf::new()),
        }
    }

    /// Sets the channel configuration used by this instance.
    pub fn with_channel_config(self, channel_config: ChannelConfig) -> Self {
        Self {
            channel_config,
            ..self
        }
    }

    /// Discovers a pixi project in the given source directory.
    pub fn discover(source_dir: &Path) -> miette::Result<Option<Self>> {
        if let Some(manifest_path) = find_pixi_manifest(source_dir) {
            let manifest = Manifest::from_path(manifest_path)?;
            return Ok(Some(Self::new(source_dir.to_path_buf(), manifest)));
        }
        Ok(None)
    }

    /// Returns the backend spec of this protocol.
    pub fn backend_tool(&self) -> ToolSpec {
        self.backend_spec.clone()
    }

    pub async fn finish(self, tool: Tool) -> Result<Protocol, InitializeError> {
        Protocol::setup(
            self.source_dir,
            self._manifest.path,
            self.channel_config,
            tool,
        )
        .await
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
