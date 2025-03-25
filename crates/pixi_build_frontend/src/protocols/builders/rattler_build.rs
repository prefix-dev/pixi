use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use miette::Diagnostic;
use rattler_conda_types::{ChannelConfig, NamedChannelOrUrl};
use thiserror::Error;

use super::pixi::ProtocolBuildError as PixiProtocolBuildError;
use crate::{
    backend_override::BackendOverride,
    protocols::{InitializeError, JsonRPCBuildProtocol},
    tool::{IsolatedToolSpec, ToolCacheError, ToolSpec},
    ToolContext,
};

const DEFAULT_BUILD_TOOL: &str = "pixi-build-rattler-build";

#[derive(Debug, Error, Diagnostic)]
pub enum FinishError {
    #[error(transparent)]
    Tool(#[from] ToolCacheError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Init(#[from] InitializeError),

    #[error("failed to setup a build backend, the project manifest at {0} does not contain a [build] section"
    )]
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

/// A builder for constructing a [`JsonRPCBuildProtocol`] instance.
#[derive(Debug)]
pub struct ProtocolBuilder {
    /// The directory that contains the source files.
    source_dir: PathBuf,

    /// The directory that contains the `recipe.yaml` in the source directory.
    recipe_dir: PathBuf,

    /// The backend tool to install.
    backend_spec: Option<ToolSpec>,

    /// The channel configuration used by this instance.
    channel_config: Option<ChannelConfig>,

    /// The cache directory the backend should use. (not used atm)
    cache_dir: Option<PathBuf>,
}

impl ProtocolBuilder {
    /// Discovers the protocol for the given source directory.
    /// We discover a `pixi.toml` file in the source directory and/or a
    /// `recipe.yaml / recipe/recipe.yaml` file.
    pub fn discover(source_dir: &Path) -> Result<Option<Self>, ProtocolBuildError> {
        let recipe_dir = source_dir.join("recipe");
        let protocol = if source_dir.join("recipe.yaml").is_file() {
            Self::new(source_dir.to_path_buf(), source_dir.to_path_buf())
        } else if recipe_dir.join("recipe.yaml").is_file() {
            Self::new(source_dir.to_path_buf(), recipe_dir)
        } else {
            return Ok(None);
        };

        Ok(Some(protocol))
    }

    /// Constructs a new instance from a manifest.
    pub fn new(source_dir: PathBuf, recipe_dir: PathBuf) -> Self {
        Self {
            source_dir,
            recipe_dir,
            backend_spec: None,
            channel_config: None,
            cache_dir: None,
        }
    }

    /// Sets a backend override.
    pub fn with_backend_override(self, backend_override: BackendOverride) -> Self {
        if let Some(overridden_tool) = backend_override.overridden_tool(DEFAULT_BUILD_TOOL) {
            Self {
                backend_spec: Some(overridden_tool.as_spec()),
                ..self
            }
        } else {
            self
        }
    }

    /// Sets the channel configuration used by this instance.
    pub fn with_channel_config(self, channel_config: ChannelConfig) -> Self {
        Self {
            channel_config: Some(channel_config),
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
        // If we have a manifest path, that means we found a `pixi.toml` file. In that
        // case we should use the backend spec from the manifest.
        let tool_spec = self.backend_spec.unwrap_or_else(|| {
            ToolSpec::Isolated(IsolatedToolSpec {
                command: DEFAULT_BUILD_TOOL.to_string(),
                specs: vec![DEFAULT_BUILD_TOOL.parse().unwrap()],
                channels: vec![
                    NamedChannelOrUrl::Name("conda-forge".to_string()),
                    NamedChannelOrUrl::Url(
                        "https://prefix.dev/pixi-build-backends".parse().unwrap(),
                    ),
                ],
            })
        });

        let channel_config = self
            .channel_config
            .unwrap_or_else(|| ChannelConfig::default_with_root_dir(self.source_dir.clone()));

        let tool = tool
            .instantiate(tool_spec, &channel_config)
            .await
            .map_err(FinishError::Tool)?;

        Ok(JsonRPCBuildProtocol::setup(
            self.source_dir,
            self.recipe_dir.join("recipe.yaml"),
            None,
            None,
            &channel_config,
            build_id,
            self.cache_dir,
            tool,
        )
        .await?)
    }
}
