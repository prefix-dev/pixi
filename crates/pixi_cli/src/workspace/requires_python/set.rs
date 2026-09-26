use clap::Parser;
use miette::IntoDiagnostic;
use pixi_core::Workspace;

#[derive(Parser, Debug)]
pub struct Args {
    /// The required Python version specifier (e.g. ">=3.10")
    #[clap(required = true, num_args = 1)]
    pub spec: String,
}

pub async fn execute(workspace: Workspace, args: Args) -> miette::Result<()> {
    let mut workspace = workspace.modify()?;

    // Set the new requires-python
    workspace
        .manifest()
        .set_requires_python(Some(args.spec.as_str()))?;

    // Save workspace
    let workspace = workspace.save().await.into_diagnostic()?;

    // Report back to the user
    let requires_python = workspace
        .workspace
        .value
        .workspace
        .requires_python
        .expect("should be set to a valid version specifier");

    eprintln!("Updated workspace requires-python to '{requires_python}'.");

    Ok(())
}
