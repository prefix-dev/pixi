//! Contains options for the build frontend
use std::path::PathBuf;

use rattler_conda_types::MatchSpec;

/// Specification of the build tool to be used
#[derive(Debug, Clone)]
pub enum BuildToolSpec {
    /// A conda package to be used as the build tool
    CondaPackage {
        /// The specs that make up the environment
        spec: MatchSpec,

        /// The command to execute in the environment
        command: Option<String>,
    },

    /// A binary to be used as the build tool
    DirectBinary(PathBuf),
    // TODO: add recipe, feedstock options here later
}

/// Build frontend options
pub struct PixiBuildFrontendOptions {
    /// Override the build tool with a specific conda package or direct binary
    pub override_build_tool: Option<BuildToolSpec>,
}
