use crate::Workspace;
use miette::IntoDiagnostic;

pub async fn execute(workspace: Workspace) -> miette::Result<()> {
    let mut workspace = workspace.modify()?;

    // Set the new workspace name
    workspace.manifest().unset_pixi_minimum()?;

    // Save workspace
    workspace.save().await.into_diagnostic()?;

    // Report back to the user
    eprintln!(
        "{}Remove workspace pixi-minimum.",
        console::style(console::Emoji("✔ ", "")).green()
    );

    Ok(())
}
