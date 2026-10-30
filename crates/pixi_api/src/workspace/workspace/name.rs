use miette::Diagnostic;
use pixi_core::{Workspace, workspace::WorkspaceMut};
use pixi_manifest::LoadManifestsError;
use thiserror::Error;

use crate::interface::Interface;
use crate::workspace::workspace::{SaveManifestError, save};

#[derive(Debug, Error, Diagnostic)]
pub enum SetNameError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    LoadWorkspace(#[from] LoadManifestsError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Save(#[from] SaveManifestError),
}

pub async fn get(workspace: &Workspace) -> String {
    workspace.display_name().to_string()
}

pub async fn set<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    name: &str,
) -> Result<(), SetNameError> {
    // Set the new workspace name
    workspace.manifest().set_name(name);

    // Save workspace
    let workspace = save(workspace).await?;

    // Report back to the user
    interface
        .success(&format!(
            "Updated workspace name to '{}'.",
            workspace.display_name()
        ))
        .await;

    Ok(())
}
