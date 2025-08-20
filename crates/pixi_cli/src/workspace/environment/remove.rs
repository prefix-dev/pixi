use clap::Parser;
use miette::IntoDiagnostic;
use pixi_core::Workspace;

#[derive(Parser, Debug, Default)]
pub struct Args {
    /// The name of the environment to remove
    pub name: String,
}

pub async fn execute(workspace: Workspace, args: Args) -> miette::Result<()> {
    let mut workspace = workspace.modify()?;

    // Remove the environment
    if !workspace.manifest().remove_environment(&args.name)? {
        // TODO: Add help for names of environments that are close.
        return Err(miette::miette!("Environment {} not found", args.name));
    }

    workspace.save().await.into_diagnostic()?;

    eprintln!(
        "{}Removed environment {}",
        console::style(console::Emoji("✔ ", "")).green(),
        args.name
    );

    Ok(())
}
