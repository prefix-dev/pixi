use clap::Parser;

use crate::Project;

/// Kill one or multiple daemon tasks of the project.
#[derive(Parser, Debug)]
pub struct Args {}

pub async fn execute(project: Project, args: Args) -> miette::Result<()> {
    println!("Hello world!");

    Ok(())
}
