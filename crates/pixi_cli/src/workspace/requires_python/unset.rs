use miette::IntoDiagnostic;
use pixi_core::Workspace;

pub async fn execute(workspace: Workspace) -> miette::Result<()> {
    let mut workspace = workspace.modify()?;

    // Unset requires-python
    workspace.manifest().set_requires_python(None)?;

    // Save workspace
    workspace.save().await.into_diagnostic()?;

    // Report back to the user
    eprintln!("Removed workspace requires-python.");

    Ok(())
}
