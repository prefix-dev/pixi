use crate::Project;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::VersionBumpType;

pub async fn execute(mut project: Project, bump_type: VersionBumpType) -> miette::Result<()> {
    // get version and exit with error if not found
    let current_version = project
        .version()
        .as_ref()
        .ok_or_else(|| miette::miette!("No version found in manifest."))?
        .clone();

    // bump version
    let new_version = current_version
        .bump(bump_type)
        .into_diagnostic()
        .context("Failed to bump version.")?;

    // Set the version
    project.manifest.set_version(&new_version.to_string())?;

    // Save the manifest on disk
    project.save()?;

    // Report back to the user
    eprintln!(
        "{}Updated project version from '{}' to '{}'.",
        console::style(console::Emoji("✔ ", "")).green(),
        current_version,
        new_version,
    );

    Ok(())
}
