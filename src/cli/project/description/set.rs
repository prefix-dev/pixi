use crate::Project;
use clap::Parser;

/// Set the project description.
#[derive(Parser, Debug, Default)]
pub struct Args {
    /// The project description
    #[clap(required = true, num_args = 1)]
    pub description: String,
}

pub async fn execute(mut project: Project, args: Args) -> miette::Result<()> {
    // Set the description
    project.manifest.set_description(args.description)?;

    // Save the manifest on disk
    project.save()?;

    // Report back to the user
    eprintln!(
        "{}Updated project description to '{}'.",
        console::style(console::Emoji("✔ ", "")).green(),
        project
            .manifest
            .parsed
            .project
            .description
            .as_ref()
            .unwrap()
    );

    Ok(())
}
