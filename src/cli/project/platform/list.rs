use crate::Project;
use clap::Parser;

/// List the platforms in the project file.
#[derive(Parser, Debug, Default)]
pub struct Args {}

pub async fn execute(project: Project, _args: Args) -> miette::Result<()> {
    project.platforms().iter().for_each(|platform| {
        eprintln!("{}", platform.as_str());
    });

    Ok(())
}
