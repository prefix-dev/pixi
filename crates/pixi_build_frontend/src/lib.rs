mod builder;
mod conda_build;
mod pixi;
mod protocol;
mod tool;

use std::{path::PathBuf, sync::Arc};

use miette::Context;
use rattler_conda_types::MatchSpec;

use crate::{builder::Builder, protocol::Protocol, tool::ToolCache};

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
pub struct Metadata {}

/// The frontend for building packages.
pub struct BuildFrontend {
    /// The cache for tools. This is used to avoid re-installing tools.
    tool_cache: Arc<ToolCache>,
}

impl Default for BuildFrontend {
    fn default() -> Self {
        Self {
            tool_cache: Arc::new(ToolCache::new()),
        }
    }
}

impl BuildFrontend {
    /// Construcst a new [`Builder`] for the given request. This object can be
    /// used to build the package.
    pub fn builder(&self, request: BuildRequest) -> miette::Result<Builder> {
        // Determine the build protocol to use for the source directory.
        let Some(protocol) = Protocol::discover(&request.source_dir)? else {
            miette::bail!(
                "could not determine how to build the package, are you missing a pixi.toml file?"
            );
        };
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

        Ok(Builder::new(protocol, tool))
    }
}
