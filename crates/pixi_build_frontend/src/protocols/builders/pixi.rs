use std::{
    fmt::{self, Display, Formatter},
    path::{Path, PathBuf},
    sync::Arc,
};

use miette::{Diagnostic, IntoDiagnostic};
use pixi_consts::consts;
use pixi_manifest::Manifest;
// pub use protocol::Protocol;
use rattler_conda_types::{ChannelConfig, ChannelUrl};
use thiserror::Error;
use which::Error;

use crate::{
    protocols::{InitializeError, JsonRPCBuildProtocol},
    tool::{IsolatedToolSpec, ToolCache, ToolCacheError, ToolSpec},
    BackendOverride, ToolContext,
};

// use super::{InitializeError, JsonRPCBuildProtocol};

/// A protocol that uses a pixi manifest to invoke a build backend .
#[derive(Debug)]
pub struct ProtocolBuilder {
    source_dir: PathBuf,
    manifest: Manifest,
    backend_spec: Option<ToolSpec>,
    backend_channels: Vec<ChannelUrl>,
    _channel_config: ChannelConfig,
    cache_dir: Option<PathBuf>,
}

#[derive(thiserror::Error, Debug, Diagnostic)]
pub enum ProtocolBuildError {
    #[error("failed to setup a build backend, the {} could not be parsed", .0.file_name().and_then(std::ffi::OsStr::to_str).unwrap_or("manifest"))]
    #[diagnostic(help("Ensure that the manifest at '{}' is a valid pixi project manifest", .0.display()))]
    FailedToParseManifest(PathBuf, #[diagnostic_source] miette::Report),

    #[error("the {} does not describe a package", .0.file_name().and_then(std::ffi::OsStr::to_str).unwrap_or("manifest"))]
    #[diagnostic(help("A [package] section is missing in the manifest"))]
    NotAPackage(PathBuf),
}

#[derive(Debug, Error, Diagnostic)]
pub enum FinishError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Init(#[from] InitializeError),
    NoBuildSection(PathBuf),
    Tool(ToolCacheError),
}

impl Display for FinishError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            FinishError::Init(init) => write!(f, "{init}"),
            FinishError::NoBuildSection(_) => write!(f, "failed to setup a build backend, the project manifest does not contain a [build-system] section"),
            FinishError::Tool(ToolCacheError::Instantiate(tool, err)) => match err {
                Error::CannotGetCurrentDirAndPathListEmpty|Error::CannotFindBinaryPath => write!(f, "failed to setup a build backend, the backend tool '{}' could not be found", tool.display()),
                Error::CannotCanonicalize => write!(f, "failed to setup a build backend, although the backend tool  '{}' can be resolved it could not be canonicalized", tool.display()),
            },
            FinishError::Tool(ToolCacheError::Install(report)) => write!(f, "failed to setup a build backend, the backend tool could not be installed: {}", report),
            FinishError::Tool(ToolCacheError::CacheDir(report)) => write!(f, "failed to setup a build backend, the cache dir could not be discovered: {}", report),
        }
    }
}

impl ProtocolBuilder {
    /// Constructs a new instance from a manifest.
    pub(crate) fn new(source_dir: PathBuf, manifest: Manifest) -> Result<Self, ProtocolBuildError> {
        let backend_spec = manifest
            .build_section()
            .map(IsolatedToolSpec::from_build_section);

        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::new());

        let backend_channels = manifest
            .build_section()
            .cloned()
            .unwrap()
            .channels
            .into_iter()
            .map(|channel| channel.into_base_url(&channel_config))
            .collect::<Result<Vec<ChannelUrl>, _>>()
            .unwrap();

        Ok(Self {
            source_dir,
            manifest,
            backend_spec: backend_spec.map(Into::into),
            backend_channels,
            _channel_config: ChannelConfig::default_with_root_dir(PathBuf::new()),
            cache_dir: None,
        })
    }

    /// Sets an optional backend override.
    pub fn with_backend_override(self, backend_override: Option<BackendOverride>) -> Self {
        Self {
            backend_spec: backend_override
                .map(BackendOverride::into_spec)
                .or(self.backend_spec),
            ..self
        }
    }

    /// Sets the channel configuration used by this instance.
    pub fn with_channel_config(self, channel_config: ChannelConfig) -> Self {
        Self {
            _channel_config: channel_config,
            ..self
        }
    }

    /// Sets the cache directory the backend should use.
    pub fn with_opt_cache_dir(self, cache_dir: Option<PathBuf>) -> Self {
        Self { cache_dir, ..self }
    }

    /// Discovers a pixi project in the given source directory.
    pub fn discover(source_dir: &Path) -> Result<Option<Self>, ProtocolBuildError> {
        if let Some(manifest_path) = find_pixi_manifest(source_dir) {
            match Manifest::from_path(&manifest_path) {
                Ok(manifest) => {
                    // Make sure the manifest describes a package.
                    if manifest.package.is_none() {
                        return Err(ProtocolBuildError::NotAPackage(manifest_path));
                    }

                    let builder = Self::new(source_dir.to_path_buf(), manifest)?;
                    return Ok(Some(builder));
                }
                Err(e) => {
                    return Err(ProtocolBuildError::FailedToParseManifest(
                        manifest_path.to_path_buf(),
                        e,
                    ));
                }
            }
        }
        Ok(None)
    }

    pub async fn finish(
        self,
        tool: Arc<ToolContext>,
        build_id: usize,
    ) -> Result<JsonRPCBuildProtocol, FinishError> {
        let tool_spec = self
            .backend_spec
            .ok_or(FinishError::NoBuildSection(self.manifest.path.clone()))?;

        let tool = tool
            .instantiate(tool_spec, &self._channel_config)
            .await
            .map_err(FinishError::Tool)?;

        Ok(JsonRPCBuildProtocol::setup(
            self.source_dir,
            self.manifest.path,
            build_id,
            self.cache_dir,
            tool,
        )
        .await?)
    }

    /// Returns the pixi manifest
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
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
