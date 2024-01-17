use clap::Parser;

/// Kill one or multiple daemon tasks of the project.
#[derive(Parser, Debug)]
pub struct Args {}

pub async fn execute(_args: Args) -> miette::Result<()> {
    println!("Hello world!");

    Ok(())
}
