use crate::Project;
use clap::Parser;

#[derive(Parser, Debug)]
pub struct Args {
    /// The project name
    #[clap(required = true, num_args = 1)]
    pub name: String,
}

pub async fn execute(mut project: Project, args: Args) -> miette::Result<()> {
    // Set the new project name
    project.manifest.set_name(&args.name)?;

    // Save project
    project.save()?;

    // Report back to the user
    eprintln!(
        "{}Updated project name to '{}'.",
        console::style(console::Emoji("✔ ", "")).green(),
        project.manifest.workspace.workspace.name
    );

    Ok(())
}
