use crate::Project;
use clap::Parser;

/// Get the project version.
#[derive(Parser, Debug, Default)]
pub struct Args {}

pub async fn execute(project: Project, _args: Args) -> miette::Result<()> {
    eprintln!("{}", project.version().as_ref().unwrap());
    Ok(())
}
