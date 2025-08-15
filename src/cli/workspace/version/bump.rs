use miette::{Context, IntoDiagnostic};
use pixi_core::Workspace;
use rattler_conda_types::VersionBumpType;

pub async fn execute(workspace: Workspace, bump_type: VersionBumpType) -> miette::Result<()> {
    let mut workspace = workspace.modify()?;

    // get version and exit with error if not found
    let current_version = workspace
        .workspace()
        .workspace
        .value
        .workspace
        .version
        .as_ref()
        .ok_or_else(|| miette::miette!("No version found in manifest."))?
        .clone();

    // bump version
    let new_version = current_version
        .bump(bump_type)
        .into_diagnostic()
        .context("Failed to bump version.")?;

    // Set the version
    workspace.manifest().set_version(&new_version.to_string())?;

    // Save the manifest on disk
    let _workspace = workspace.save().await.into_diagnostic()?;

    // Report back to the user
    eprintln!(
        "{}Updated workspace version from '{}' to '{}'.",
        console::style(console::Emoji("✔ ", "")).green(),
        current_version,
        new_version,
    );

    Ok(())
}
