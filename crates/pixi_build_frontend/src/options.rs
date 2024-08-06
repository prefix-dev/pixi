//! Contains options for the build frontend
use rattler_conda_types::MatchSpec;
use std::path::PathBuf;

/// Specification of the build tool to be used
#[derive(Debug, Clone)]
pub enum BuildToolSpec {
    /// A conda package to be used as the build tool
    CondaPackage(MatchSpec),
    /// A binary to be used as the build tool
    DirectBinary(PathBuf),
    // TODO: add recipe, feedstock options here later
}

/// Build frontend options
pub struct PixiBuildFrontendOptions {
    /// Override the build tool with a specific conda package or direct binary
    pub override_build_tool: Option<BuildToolSpec>,
}
