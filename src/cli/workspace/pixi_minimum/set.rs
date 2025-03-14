use crate::Workspace;
use clap::Parser;
use miette::IntoDiagnostic;

#[derive(Parser, Debug)]
pub struct Args {
    /// The required pixi version
    #[clap(required = true, num_args = 1)]
    pub version: String,
}

pub async fn execute(workspace: Workspace, args: Args) -> miette::Result<()> {
    let mut workspace = workspace.modify()?;

    // Set the new workspace name
    workspace
        .manifest()
        .set_pixi_minimum(args.version.as_str())?;

    // Save workspace
    let workspace = workspace.save().await.into_diagnostic()?;

    // Report back to the user
    eprintln!(
        "{}Updated workspace pixi-minimum to '{}'.",
        console::style(console::Emoji("✔ ", "")).green(),
        workspace
            .workspace
            .value
            .workspace
            .pixi_minimum
            .expect("should be set a valid version")
    );

    Ok(())
}
