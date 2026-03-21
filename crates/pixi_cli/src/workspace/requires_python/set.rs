use clap::Parser;
use miette::IntoDiagnostic;
use pixi_core::Workspace;

#[derive(Parser, Debug)]
pub struct Args {
    /// The required python version specifier (e.g. ">=3.10")
    #[clap(required = true, num_args = 1)]
    pub version: String,
}

pub async fn execute(workspace: Workspace, args: Args) -> miette::Result<()> {
    let mut workspace = workspace.modify()?;

    // Set the new requires-python version
    workspace
        .manifest()
        .set_requires_python(Some(args.version.as_str()))?;

    // Save workspace
    workspace.save().await.into_diagnostic()?;

    // Report back to the user
    eprintln!(
        "{}Updated workspace requires-python to '{}'.",
        console::style(console::Emoji("✔ ", "")).green(),
        args.version
    );

    Ok(())
}
