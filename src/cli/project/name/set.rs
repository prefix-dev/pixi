use crate::Workspace;
use clap::Parser;
use miette::IntoDiagnostic;

#[derive(Parser, Debug)]
pub struct Args {
    /// The project name
    #[clap(required = true, num_args = 1)]
    pub name: String,
}

pub async fn execute(workspace: Workspace, args: Args) -> miette::Result<()> {
    let mut workspace = workspace.modify()?;

    // Set the new project name
    workspace.manifest().set_name(&args.name)?;

    // Save workspace
    let workspace = workspace.save().await.into_diagnostic()?;

    // Report back to the user
    eprintln!(
        "{}Updated workspace name to '{}'.",
        console::style(console::Emoji("✔ ", "")).green(),
        workspace.workspace.value.workspace.name
    );

    Ok(())
}
