use miette::IntoDiagnostic;
use pixi_core::{Workspace, workspace::WorkspaceMut};

use crate::interface::Interface;

pub async fn get(workspace: &Workspace) -> String {
    workspace.display_name().to_string()
}

pub async fn set<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    name: &str,
) -> miette::Result<()> {
    // Set the new workspace name
    workspace.manifest().set_name(name)?;

    // Save workspace
    let workspace = workspace.save().await.into_diagnostic()?;

    // Report back to the user
    interface
        .success(&format!(
            "Updated workspace name to '{}'.",
            workspace.display_name()
        ))
        .await;

    Ok(())
}
