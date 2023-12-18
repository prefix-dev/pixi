use crate::Project;
use clap::Parser;

/// Get the project version.
#[derive(Parser, Debug, Default)]
pub struct Args {}

pub async fn execute(project: Project, _args: Args) -> miette::Result<()> {
    // Print the version if it exists
    if let Some(version) = project.version() {
        eprintln!("{}", version);
    }
    Ok(())
}
