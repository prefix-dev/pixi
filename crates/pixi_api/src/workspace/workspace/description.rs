use miette::IntoDiagnostic;
use pixi_core::{Workspace, workspace::WorkspaceMut};

use crate::Interface;

pub async fn get(workspace: &Workspace) -> Option<String> {
    workspace.workspace.value.workspace.description.clone()
}

pub async fn set<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    description: &str,
) -> miette::Result<()> {
    // Set the description
    workspace.manifest().set_description(description)?;

    // Save the manifest on disk
    let _ = workspace.save().await.into_diagnostic()?;

    // Report back to the user
    interface
        .success(&format!(
            "Updated workspace description to '{description}'.",
        ))
        .await;

    Ok(())
}
