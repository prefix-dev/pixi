use crate::Workspace;
use miette::IntoDiagnostic;

pub async fn execute(workspace: Workspace) -> miette::Result<()> {
    let mut workspace = workspace.modify()?;

    // Set the new workspace name
    workspace.manifest().unset_requires_pixi()?;

    // Save workspace
    workspace.save().await.into_diagnostic()?;

    // Report back to the user
    eprintln!(
        "{}Remove workspace requires-pixi.",
        console::style(console::Emoji("✔ ", "")).green()
    );

    Ok(())
}
