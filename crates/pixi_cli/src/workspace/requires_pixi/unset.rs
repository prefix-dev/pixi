use miette::IntoDiagnostic;
use pixi_core::Workspace;

pub async fn execute(workspace: Workspace) -> miette::Result<()> {
    let mut workspace = workspace.modify()?;

    // Set the new workspace name
    workspace.manifest().set_requires_pixi(None)?;

    // Save workspace
    workspace.save().await.into_diagnostic()?;

    // Report back to the user
    eprintln!(
        "{}Remove workspace requires-pixi.",
        console::style(console::Emoji("✔ ", "")).green()
    );

    Ok(())
}
