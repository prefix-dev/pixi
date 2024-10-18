mod protocol;
mod stderr;

use std::{
    fmt,
    fmt::{Display, Formatter},
    path::{Path, PathBuf},
};

use miette::Diagnostic;
use pixi_consts::consts;
use pixi_manifest::Manifest;
pub use protocol::{InitializeError, Protocol};
use rattler_conda_types::ChannelConfig;
pub(crate) use stderr::{stderr_null, stderr_stream};
use thiserror::Error;
use which::Error;

use crate::{
    tool::{IsolatedToolSpec, ToolCache, ToolCacheError, ToolSpec},
    BackendOverride,
};

/// A protocol that uses a pixi manifest to invoke a build backend .
#[derive(Debug)]
pub(crate) struct ProtocolBuilder {
    source_dir: PathBuf,
    manifest: Manifest,
    backend_spec: Option<ToolSpec>,
    channel_config: ChannelConfig,
    cache_dir: Option<PathBuf>,
}

#[derive(thiserror::Error, Debug, Diagnostic)]
pub enum ProtocolBuildError {
    #[error("failed to setup a build backend, the {} could not be parsed", .0.file_name().and_then(std::ffi::OsStr::to_str).unwrap_or("manifest"))]
    #[diagnostic(help("Ensure that the manifest at '{}' is a valid pixi project manifest", .0.display()))]
    FailedToParseManifest(PathBuf, #[diagnostic_source] miette::Report),
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
            FinishError::NoBuildSection(_) => write!(f, "failed to setup a build backend, the project manifest does not contain a [build] section"),
            FinishError::Tool(ToolCacheError::Instantiate(tool, err)) => match err {
                Error::CannotGetCurrentDirAndPathListEmpty|Error::CannotFindBinaryPath => write!(f, "failed to setup a build backend, the backend tool '{}' could not be found", tool.display()),
                Error::CannotCanonicalize => write!(f, "failed to setup a build backend, although the backend tool  '{}' can be resolved it could not be canonicalized", tool.display()),
            }
        }
    }
}

impl ProtocolBuilder {
    /// Constructs a new instance from a manifest.
    pub(crate) fn new(source_dir: PathBuf, manifest: Manifest) -> Result<Self, ProtocolBuildError> {
        let backend_spec = manifest
            .build_section()
            .map(IsolatedToolSpec::from_build_section);

        Ok(Self {
            source_dir,
            manifest,
            backend_spec: backend_spec.map(Into::into),
            channel_config: ChannelConfig::default_with_root_dir(PathBuf::new()),
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
            channel_config,
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

    pub async fn finish(self, tool: &ToolCache, build_id: usize) -> Result<Protocol, FinishError> {
        let tool_spec = self
            .backend_spec
            .ok_or(FinishError::NoBuildSection(self.manifest.path.clone()))?;
        let tool = tool.instantiate(tool_spec).map_err(FinishError::Tool)?;
        Ok(Protocol::setup(
            self.source_dir,
            self.manifest.path,
            build_id,
            self.cache_dir,
            self.channel_config,
            tool,
        )
        .await?)
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
