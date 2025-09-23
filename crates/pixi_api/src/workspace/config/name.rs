use miette::IntoDiagnostic;
use pixi_core::Workspace;

use crate::interface::Interface;

pub(crate) async fn get(workspace: Workspace) -> String {
    workspace.display_name().to_string()
}

pub(crate) async fn set<I: Interface>(
    interface: &I,
    workspace: Workspace,
    name: &str,
) -> miette::Result<()> {
    let mut workspace = workspace.modify()?;

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
