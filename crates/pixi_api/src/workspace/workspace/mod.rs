use std::path::PathBuf;

use futures::TryFutureExt;
use miette::Diagnostic;
use pixi_core::{Workspace, workspace::WorkspaceMut};
use thiserror::Error;

pub mod activation;
pub mod channel;
pub mod description;
pub mod environment;
pub mod feature;
pub mod name;
pub mod platform;
pub mod preview;

/// The edited manifest could not be written back to disk.
#[derive(Debug, Error, Diagnostic)]
#[error("failed to save '{}'", .path.display())]
pub struct SaveManifestError {
    pub path: PathBuf,
    #[source]
    pub source: std::io::Error,
}

/// Saves `workspace`, naming the manifest in the error since the io error
/// doesn't always carry the path.
///
/// Not `async`: that would store the `save` future twice, and it is large.
fn save(workspace: WorkspaceMut) -> impl Future<Output = Result<Workspace, SaveManifestError>> {
    let path = workspace.workspace().workspace.provenance.path.clone();
    workspace
        .save()
        .map_err(move |source| SaveManifestError { path, source })
}
