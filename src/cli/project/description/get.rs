use crate::Project;
use clap::Parser;

/// Get the project description.
#[derive(Parser, Debug, Default)]
pub struct Args {}

pub async fn execute(project: Project, _args: Args) -> miette::Result<()> {
    // Print the description if it exists
    if let Some(description) = project.description() {
        eprintln!("{}", description);
    }
    Ok(())
}
