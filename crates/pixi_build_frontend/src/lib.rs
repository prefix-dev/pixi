mod conda_build;
mod jsonrpc;
pub mod pixi;
pub mod protocol;
mod tool;

use std::{path::PathBuf, sync::Arc};

use miette::Context;
use pixi_manifest::Dependencies;
use pixi_spec::PixiSpec;
use rattler_conda_types::{
    ChannelConfig, MatchSpec, NoArchType, PackageName, Platform, VersionWithSource,
};
pub use tool::{IsolatedToolSpec, SystemToolSpec, ToolSpec};
use url::Url;

pub use crate::protocol::Protocol;
use crate::{protocol::ProtocolBuilder, tool::ToolCache};

#[derive(Debug, Clone)]
pub struct BackendOverrides {
    /// The specs to use for the build tool.
    pub spec: Option<MatchSpec>,

    /// Path to a system build tool.
    pub path: Option<PathBuf>,
}

#[derive(Debug)]
pub struct BuildRequest {
    /// The source directory that contains the source package.
    pub source_dir: PathBuf,

    /// Overrides for the build tool.
    pub build_tool_overrides: BackendOverrides,
}

#[derive(Debug)]
pub struct BuildOutput {
    /// Paths to the built artifacts.
    pub artifacts: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct CondaMetadata {
    /// Metadata of all the package that can be built from the source directory.
    pub packages: Vec<CondaPackageMetadata>,
}

#[derive(Debug)]
pub struct CondaPackageMetadata {
    /// The name of the package
    pub name: PackageName,

    /// The version of the package
    pub version: VersionWithSource,

    /// The build hash of the package
    pub build: String,

    /// The build number of the package
    pub build_number: u64,

    /// The subdirectory the package would be placed in
    pub subdir: Platform,

    /// The dependencies of the package
    pub depends: Dependencies<PackageName, PixiSpec>,

    /// Constraints of the package
    pub constraints: Dependencies<PackageName, PixiSpec>,

    /// The license of the package
    pub license: Option<String>,

    /// The license family of the package
    pub license_family: Option<String>,

    /// Whether this is a noarch package
    pub noarch: NoArchType,
}

#[derive(Debug)]
pub struct CondaMetadataRequest {
    /// The base urls of the channels to use.
    pub channel_base_urls: Vec<Url>,
}

/// The frontend for building packages.
pub struct BuildFrontend {
    /// The cache for tools. This is used to avoid re-installing tools.
    tool_cache: Arc<ToolCache>,

    /// The channel configuration used by the frontend
    channel_config: ChannelConfig,
}

impl Default for BuildFrontend {
    fn default() -> Self {
        Self {
            tool_cache: Arc::new(ToolCache::new()),
            channel_config: ChannelConfig::default_with_root_dir(PathBuf::new()),
        }
    }
}

impl BuildFrontend {
    /// Specify the channel configuration
    pub fn with_channel_config(self, channel_config: ChannelConfig) -> Self {
        Self {
            channel_config,
            ..self
        }
    }

    /// Returns the channel config of the frontend
    pub fn channel_config(&self) -> &ChannelConfig {
        &self.channel_config
    }

    /// Construcst a new [`Builder`] for the given request. This object can be
    /// used to build the package.
    pub async fn protocol(&self, request: BuildRequest) -> miette::Result<Protocol> {
        // Determine the build protocol to use for the source directory.
        let protocol = ProtocolBuilder::discover(&request.source_dir)?
            .ok_or_else(|| {
                miette::miette!("could not determine how to build the package, are you missing a pixi.toml file?")
            })?
            .with_channel_config(self.channel_config.clone());

        tracing::info!(
            "discovered a {} source package at {}",
            protocol.name(),
            request.source_dir.display()
        );

        // Instantiate the build tool.
        let tool_spec = request
            .build_tool_overrides
            .into_spec()
            .unwrap_or(protocol.backend_tool());
        let tool = self
            .tool_cache
            .instantiate(&tool_spec)
            .context("failed to instantiate build tool")?;

        protocol.finish(tool).await
    }
}
