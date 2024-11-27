use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use miette::Diagnostic;
use pixi_manifest::Manifest;

// pub use protocol::Protocol;
use rattler_conda_types::ChannelConfig;
use thiserror::Error;

use super::pixi::{self, ProtocolBuildError as PixiProtocolBuildError};

use crate::{
    protocols::{InitializeError, JsonRPCBuildProtocol},
    tool::{IsolatedToolSpec, ToolCacheError, ToolSpec},
    BackendOverride, ToolContext,
};

const DEFAULT_BUILD_TOOL: &str = "pixi-build-rattler-build";

#[derive(Debug, Error, Diagnostic)]
pub enum FinishError {
    #[error(transparent)]
    Tool(#[from] ToolCacheError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Init(#[from] InitializeError),

    #[error("failed to setup a build backend, the project manifest at {0} does not contain a [build] section")]
    NoBuildSection(PathBuf),
}

/// Right now building a rattler-build protocol is *almost* infallible.
/// The only way it can fail is if the pixi protocol cannot be built.
/// This error for now is mostly a wrapper around the pixi protocol build error.
#[derive(thiserror::Error, Debug, Diagnostic)]
pub enum ProtocolBuildError {
    #[error(transparent)]
    FailedToBuildPixi(#[from] PixiProtocolBuildError),
}

/// A builder for constructing a [`protocol::Protocol`] instance.
#[derive(Debug)]
pub struct ProtocolBuilder {
    /// The directory that contains the source files.
    source_dir: PathBuf,

    /// The directory that contains the `recipe.yaml` in the source directory.
    recipe_dir: PathBuf,

    /// The path to the manifest file.
    manifest_path: Option<PathBuf>,

    /// The backend tool to install.
    backend_spec: Option<ToolSpec>,

    /// The channel configuration used by this instance.
    _channel_config: ChannelConfig,

    /// The cache directory the backend should use. (not used atm)
    cache_dir: Option<PathBuf>,
}

impl ProtocolBuilder {
    /// Discovers the protocol for the given source directory.
    /// We discover a `pixi.toml` file in the source directory and/or a `recipe.yaml / recipe/recipe.yaml` file.
    pub fn discover(source_dir: &Path) -> Result<Option<Self>, ProtocolBuildError> {
        // Ignore the error if we cannot find the pixi protocol.
        let manifest = match pixi::ProtocolBuilder::discover(source_dir) {
            Ok(protocol) => protocol.map(|protocol| protocol.manifest().clone()),
            Err(_) => None,
        };

        let recipe_dir = source_dir.join("recipe");

        let protocol = if source_dir.join("recipe.yaml").is_file() {
            Self::new(source_dir, source_dir, manifest)
        } else if recipe_dir.join("recipe.yaml").is_file() {
            Self::new(source_dir, &recipe_dir, manifest)
        } else {
            return Ok(None);
        };

        Ok(Some(protocol))
    }

    /// Constructs a new instance from a manifest.
    pub fn new(source_dir: &Path, recipe_dir: &Path, manifest: Option<Manifest>) -> Self {
        let backend_spec = if let Some(manifest) = &manifest {
            manifest
                .build_section()
                .map(IsolatedToolSpec::from_build_section)
        } else {
            None
        };

        Self {
            source_dir: source_dir.to_path_buf(),
            recipe_dir: recipe_dir.to_path_buf(),
            manifest_path: manifest.map(|m| m.path.clone()),
            backend_spec: backend_spec.map(Into::into),
            _channel_config: ChannelConfig::default_with_root_dir(PathBuf::new()),
            cache_dir: None,
        }
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

    /// Create the protocol instance.
    pub async fn finish(
        self,
        tool: Arc<ToolContext>,
        build_id: usize,
    ) -> Result<JsonRPCBuildProtocol, FinishError> {
        // If we have a manifest path, that means we found a `pixi.toml` file. In that case
        // we should use the backend spec from the manifest.
        let tool_spec = if let Some(manifest_path) = &self.manifest_path {
            self.backend_spec
                .ok_or(FinishError::NoBuildSection(manifest_path.clone()))?
        } else {
            ToolSpec::Isolated(
                IsolatedToolSpec::from_specs([DEFAULT_BUILD_TOOL.parse().unwrap()])
                    .with_command(DEFAULT_BUILD_TOOL),
            )
        };

        let tool = tool
            .instantiate(tool_spec, &self._channel_config)
            .await
            .map_err(FinishError::Tool)?;

        Ok(JsonRPCBuildProtocol::setup(
            self.source_dir,
            self.recipe_dir.join("recipe.yaml"),
            build_id,
            self.cache_dir,
            tool,
        )
        .await?)
    }
}
