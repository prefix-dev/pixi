use clap::Parser;

use crate::Workspace;

#[derive(Parser, Debug, Default)]
pub struct Args {
    /// The name of the environment to remove
    pub name: String,
}

pub async fn execute(mut project: Workspace, args: Args) -> miette::Result<()> {
    // Remove the environment
    if !project.manifest.remove_environment(&args.name)? {
        // TODO: Add help for names of environments that are close.
        return Err(miette::miette!("Environment {} not found", args.name));
    }

    project.save()?;

    eprintln!(
        "{}Removed environment {}",
        console::style(console::Emoji("✔ ", "")).green(),
        args.name
    );

    Ok(())
}
