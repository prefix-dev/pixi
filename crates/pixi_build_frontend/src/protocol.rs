use std::path::PathBuf;

use miette::Diagnostic;

use crate::protocols::builders::{pixi_protocol, rattler_build_protocol};

/// Top-level error type for protocol errors.
#[derive(Debug, thiserror::Error, Diagnostic)]
pub enum FinishError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Pixi(#[from] pixi_protocol::FinishError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    RattlerBuild(#[from] rattler_build_protocol::FinishError),
}

#[derive(Debug, thiserror::Error, Diagnostic)]
pub enum DiscoveryError {
    #[error(
        "failed to discover a valid project manifest, the source does not refer to a directory"
    )]
    NotADirectory,

    #[error("failed to discover a valid project manifest, the source path '{}' could not be found", .0.display())]
    NotFound(PathBuf),

    #[error("the source directory does not contain a supported manifest")]
    #[diagnostic(help(
        "Ensure that the source directory contains a valid pixi.toml or meta.yaml file."
    ))]
    UnsupportedFormat,

    #[error(transparent)]
    #[diagnostic(transparent)]
    Pixi(#[from] pixi_protocol::ProtocolBuildError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    RattlerBuild(#[from] rattler_build_protocol::ProtocolBuildError),
}
