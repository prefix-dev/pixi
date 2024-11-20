mod protocol;
mod stderr;

use std::{
    convert::Infallible,
    path::{Path, PathBuf},
};

use miette::Diagnostic;
use protocol::InitializeError;
pub use protocol::Protocol;
use rattler_conda_types::{ChannelConfig, MatchSpec, ParseStrictness::Strict};
use thiserror::Error;

use crate::{
    tool::{IsolatedToolSpec, ToolCache, ToolCacheError, ToolSpec},
    BackendOverride,
};

#[derive(Debug, Error, Diagnostic)]
pub enum FinishError {
    #[error(transparent)]
    Tool(#[from] ToolCacheError),
    #[error(transparent)]
    #[diagnostic(transparent)]
    Init(#[from] InitializeError),
}

/// A builder for constructing a [`protocol::Protocol`] instance.
#[derive(Debug)]
pub struct ProtocolBuilder {
    /// The directory that contains the source files.
    source_dir: PathBuf,

    /// The directory that contains the `recipe.yaml` in the source directory.
    recipe_dir: PathBuf,

    /// The backend tool to install.
    backend_spec: ToolSpec,

    /// The channel configuration used by this instance.
    channel_config: ChannelConfig,

    /// The cache directory the backend should use. (not used atm)
    _cache_dir: Option<PathBuf>,

    /// A user friendly name for the backend.
    backend_identifier: String,
}

impl ProtocolBuilder {
    /// Discovers the protocol for the given source directory.
    pub fn discover(source_dir: &Path) -> miette::Result<Option<Self>> {
        let recipe_dir = source_dir.join("recipe");

        let protocol = if source_dir.join("recipe.yaml").is_file() {
            Self::new(source_dir, source_dir)
        } else if recipe_dir.join("recipe.yaml").is_file() {
            Self::new(source_dir, &recipe_dir)
        } else {
            return Ok(None);
        };

        Ok(Some(protocol))
    }

    /// Constructs a new instance from a manifest.
    pub fn new(source_dir: &Path, recipe_dir: &Path) -> Self {
        let backend_spec = IsolatedToolSpec::from_specs(vec![MatchSpec::from_str(
            "pixi-build-rattler-build",
            Strict,
        )
        .unwrap()])
        .with_command("pixi-build-rattler-build")
        .into();

        Self {
            source_dir: source_dir.to_path_buf(),
            recipe_dir: recipe_dir.to_path_buf(),
            backend_spec,
            channel_config: ChannelConfig::default_with_root_dir(PathBuf::new()),
            _cache_dir: None,
            backend_identifier: "pixi-build-rattler-build".to_string(),
        }
    }

    /// Sets an optional backend override.
    pub fn with_backend_override(self, backend_override: Option<BackendOverride>) -> Self {
        Self {
            backend_spec: backend_override
                .map(BackendOverride::into_spec)
                .unwrap_or(self.backend_spec),
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
        Self {
            _cache_dir: cache_dir,
            ..self
        }
    }

    pub async fn finish(self, tool: &ToolCache, build_id: usize) -> Result<Protocol, FinishError> {
        let tool_spec = self.backend_spec;

        let tool = tool
            .instantiate(tool_spec)
            .await
            .map_err(FinishError::Tool)?;

        Ok(Protocol::setup(
            self.source_dir,
            self.recipe_dir,
            build_id,
            self._cache_dir,
            self.channel_config,
            tool,
        )
        .await?)
    }
}
