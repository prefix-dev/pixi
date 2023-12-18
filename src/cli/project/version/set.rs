use crate::Project;
use clap::Parser;

/// Set the project version.
#[derive(Parser, Debug, Default)]
pub struct Args {
    /// The new project version
    #[clap(required = true, num_args = 1)]
    pub version: String,
}

pub async fn execute(mut project: Project, args: Args) -> miette::Result<()> {
    // Set the version
    project.manifest.set_version(args.version)?;

    // Save the manifest on disk
    project.save()?;

    // Report back to the user
    eprintln!(
        "{}Updated project version to '{}'.",
        console::style(console::Emoji("✔ ", "")).green(),
        project.version().as_ref().unwrap()
    );

    Ok(())
}
