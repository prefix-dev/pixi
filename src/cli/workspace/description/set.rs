use pixi_core::Workspace;
use clap::Parser;
use miette::IntoDiagnostic;

#[derive(Parser, Debug, Default)]
pub struct Args {
    /// The workspace description
    #[clap(required = true, num_args = 1)]
    pub description: String,
}

pub async fn execute(workspace: Workspace, args: Args) -> miette::Result<()> {
    let mut workspace = workspace.modify()?;

    // Set the description
    workspace.manifest().set_description(&args.description)?;

    // Save the manifest on disk
    let workspace = workspace.save().await.into_diagnostic()?;

    // Report back to the user
    eprintln!(
        "{}Updated workspace description to '{}'.",
        console::style(console::Emoji("✔ ", "")).green(),
        workspace
            .workspace
            .value
            .workspace
            .description
            .as_ref()
            .expect("we just set the description, so it should be there")
    );

    Ok(())
}
