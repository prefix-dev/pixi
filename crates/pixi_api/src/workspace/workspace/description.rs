use miette::Diagnostic;
use pixi_core::{Workspace, workspace::WorkspaceMut};
use pixi_manifest::LoadManifestsError;
use thiserror::Error;

use crate::Interface;
use crate::workspace::workspace::{SaveManifestError, save};

#[derive(Debug, Error, Diagnostic)]
pub enum SetDescriptionError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    LoadWorkspace(#[from] LoadManifestsError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Save(#[from] SaveManifestError),
}

pub async fn get(workspace: &Workspace) -> Option<String> {
    workspace.workspace.value.workspace.description.clone()
}

pub async fn set<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    description: &str,
) -> Result<(), SetDescriptionError> {
    // Set the description
    workspace.manifest().set_description(description);

    // Save the manifest on disk
    save(workspace).await?;

    // Report back to the user
    interface
        .success(&format!(
            "Updated workspace description to '{description}'.",
        ))
        .await;

    Ok(())
}
