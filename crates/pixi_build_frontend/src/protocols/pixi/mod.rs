mod protocol;

use std::path::{Path, PathBuf};

use miette::IntoDiagnostic;
use pixi_consts::consts;
use pixi_manifest::Manifest;
pub use protocol::{InitializeError, Protocol};
use rattler_conda_types::ChannelConfig;

use crate::tool::{IsolatedToolSpec, Tool, ToolSpec};

/// A protocol that uses a pixi manifest to invoke a build backend .
#[derive(Debug)]
pub(crate) struct ProtocolBuilder {
    source_dir: PathBuf,
    manifest: Manifest,
    backend_spec: ToolSpec,
    channel_config: ChannelConfig,
}

#[derive(thiserror::Error, Debug)]
pub enum ProtocolBuildError {
    #[error("No build section found")]
    NoBuildSection,
}

impl ProtocolBuilder {
    /// Constructs a new instance from a manifest.
    pub(crate) fn new(source_dir: PathBuf, manifest: Manifest) -> Result<Self, ProtocolBuildError> {
        let backend_spec = manifest
            .build_section()
            .map(IsolatedToolSpec::from_build_section)
            .ok_or(ProtocolBuildError::NoBuildSection)?;

        Ok(Self {
            source_dir,
            manifest,
            backend_spec: backend_spec.into(),
            channel_config: ChannelConfig::default_with_root_dir(PathBuf::new()),
        })
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
            return Ok(Some(
                Self::new(source_dir.to_path_buf(), manifest).into_diagnostic()?,
            ));
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
            self.manifest.path,
            self.manifest.parsed.project.name,
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::ProtocolBuilder;

    #[test]
    pub fn discover_basic_pixi_manifest() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/basic");
        let manifest_path = super::find_pixi_manifest(&manifest_dir)
            .unwrap_or_else(|| panic!("No manifest found at {}", manifest_dir.display()));
        ProtocolBuilder::discover(&manifest_path).unwrap();
    }
}
