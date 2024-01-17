use clap::Parser;

/// List all the daemon tasks of the project.
#[derive(Parser, Debug)]
pub struct Args {}

pub async fn execute(_args: Args) -> miette::Result<()> {
    println!("Hello world!");

    Ok(())
}
