//! Compute-engine integration for the PyPI pipeline.
//!
//! This crate exposes the PyPI resolve and install operations as extension
//! traits on [`pixi_compute_engine::ComputeCtx`] (with convenience wrappers
//! on [`pixi_compute_engine::ComputeEngine`]). The heavy lifting lives in
//! `pixi_install_pypi`; this crate wires it to engine-wide shared state:
//!
//! - the [`pixi_uv_context::UvResolutionContext`] and the workspace
//!   [`pixi_compute_sources::RootDir`] come from the engine's
//!   [`DataStore`](pixi_compute_engine::DataStore),
//! - progress is reported through [`SolvePypiReporter`] and
//!   [`InstallPypiReporter`] objects registered in the same store, mirroring
//!   the reporter mechanism of the conda solve and install paths.

mod data;
mod install;
mod reporter;
mod solve;

pub use data::{HasUvResolutionContext, UvResolutionContextSource};
pub use install::{InstallPypiEnvironmentExt, InstallPypiEnvironmentSpec};
pub use reporter::{
    HasInstallPypiReporter, HasSolvePypiReporter, InstallPypiReporter, SolvePypiReporter,
};
pub use solve::{SolvePypiEnvironmentExt, SolvePypiEnvironmentSpec};

// Re-export the types callers need to build the specs and implement the
// reporters.
pub use pixi_install_pypi::{
    InstallablePypiRecord, LazyEnvironmentVariables, LockedPypiRecord, ManifestData,
    UnresolvedPypiRecord,
    resolve::{CondaPrefixProvider, ProvidedCondaPrefix},
};
pub use pixi_uv_reporter::{UvReporter, UvReporterOptions};
