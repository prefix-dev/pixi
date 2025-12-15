use miette::IntoDiagnostic;
use pixi_core::{Workspace, workspace::WorkspaceMut};

use crate::Interface;

pub async fn get(workspace: &Workspace) -> Option<String> {
    workspace.workspace.value.workspace.description.clone()
}

pub async fn set<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    description: Option<String>,
) -> miette::Result<()> {
    // Set the description
    workspace.manifest().set_description(description)?;

    // Save the manifest on disk
    let workspace = workspace.save().await.into_diagnostic()?;

    // Report back to the user
    interface
        .success(&format!(
            "Updated workspace description to '{}'.",
            workspace
                .workspace
                .value
                .workspace
                .description
                .as_ref()
                .expect("we just set the description, so it should be there")
        ))
        .await;

    Ok(())
}
