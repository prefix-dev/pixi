use crate::Project;
use clap::Parser;

pub enum BumpType {
    Major,
    Minor,
    Patch,
}

/// Bump the project version.
#[derive(Parser, Debug, Default)]
pub struct Args {}

pub async fn execute(bump_type: BumpType, mut project: Project, _args: Args) -> miette::Result<()> {
    let current_version = project.version().as_ref().unwrap().clone();

    // NOTE(hadim): logic is the same everywhere (bump last component)
    // The correct logic should be added on rattler probably.
    let new_version = match bump_type {
        BumpType::Major => current_version.bump(),
        BumpType::Minor => current_version.bump(),
        BumpType::Patch => current_version.bump(),
    };

    // Set the version
    project.manifest.set_version(&new_version.to_string())?;

    // Save the manifest on disk
    project.save()?;

    // Report back to the user
    eprintln!(
        "{}Bump project version from '{}' to '{}'.",
        console::style(console::Emoji("✔ ", "")).green(),
        current_version,
        new_version
    );

    Ok(())
}
