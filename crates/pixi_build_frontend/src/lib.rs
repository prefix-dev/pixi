mod backend;
mod conda_build;
mod pixi;
mod protocol;

use std::path::PathBuf;

use rattler_conda_types::MatchSpec;

use crate::protocol::Protocol;

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

/// The frontend for building packages.
pub struct BuildFrontend {
    // TODO: Add caches?
}

impl Default for BuildFrontend {
    fn default() -> Self {
        Self {}
    }
}

impl BuildFrontend {
    pub fn build(&self, request: BuildRequest) -> miette::Result<BuildOutput> {
        // Determine the build protocol to use for the source directory.
        let Some(protocol) = Protocol::discover(&request.source_dir, request.build_tool_overrides)?
        else {
            miette::bail!(
                "could not determine how to build the package, are you missing a pixi.toml file?"
            );
        };
        tracing::info!(
            "discovered a {} source package at {}",
            protocol.name(),
            request.source_dir.display()
        );

        // Determine the build backend to use for the protocol.
        let backend_spec = protocol.backend_spec();

        // TODO: Instantiate the build backend

        // TODO: Invoke the build backend through the protocol.

        Ok(BuildOutput { artifacts: vec![] })
    }
}
