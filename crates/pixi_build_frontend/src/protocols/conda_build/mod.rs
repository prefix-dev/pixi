mod protocol;

use std::convert::Infallible;
use std::path::{Path, PathBuf};

use rattler_conda_types::{ChannelConfig, MatchSpec, ParseStrictness::Strict};

use crate::tool::{IsolatedToolSpec, Tool, ToolSpec};

pub use protocol::Protocol;

/// A builder for constructing a [`protocol::Protocol`] instance.
#[derive(Debug, Clone)]
pub struct ProtocolBuilder {
    /// The directory that contains the source files.
    source_dir: PathBuf,

    /// The directory that contains the `meta.yaml` in the source directory.
    recipe_dir: PathBuf,

    /// The backend tool to install.
    backend_spec: ToolSpec,

    /// The channel configuration used by this instance.
    channel_config: ChannelConfig,
}

impl ProtocolBuilder {
    /// Discovers the protocol for the given source directory.
    pub fn discover(source_dir: &Path) -> Result<Option<Self>, Infallible> {
        let recipe_dir = source_dir.join("recipe");
        let protocol = if source_dir.join("meta.yaml").is_file() {
            Self::new(source_dir, source_dir)
        } else if recipe_dir.join("meta.yaml").is_file() {
            Self::new(source_dir, &recipe_dir)
        } else {
            return Ok(None);
        };

        Ok(Some(protocol))
    }

    /// Constructs a new instance from a manifest.
    pub fn new(source_dir: &Path, recipe_dir: &Path) -> Self {
        let backend_spec =
            IsolatedToolSpec::from_specs(vec![MatchSpec::from_str("conda-build", Strict).unwrap()])
                .into();

        Self {
            source_dir: source_dir.to_path_buf(),
            recipe_dir: recipe_dir.to_path_buf(),
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

    /// Information about the backend tool to install.
    pub fn backend_tool(&self) -> ToolSpec {
        self.backend_spec.clone()
    }

    pub fn finish(self, tool: Tool) -> Protocol {
        Protocol {
            channel_config: self.channel_config,
            tool,
            _source_dir: self.source_dir,
            recipe_dir: self.recipe_dir,
        }
    }
}
