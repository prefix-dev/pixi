use crate::Workspace;
use clap::Parser;
use miette::IntoDiagnostic;

#[derive(Parser, Debug, Default)]
pub struct Args {
    /// The new project version
    #[clap(required = true, num_args = 1)]
    pub version: String,
}

pub async fn execute(workspace: Workspace, args: Args) -> miette::Result<()> {
    let mut workspace = workspace.modify()?;

    // Set the version
    workspace.manifest().set_version(&args.version)?;

    // Save the manifest on disk
    let workspace = workspace.save().await.into_diagnostic()?;

    // Report back to the user
    eprintln!(
        "{}Updated project version to '{}'.",
        console::style(console::Emoji("✔ ", "")).green(),
        workspace
            .workspace
            .value
            .workspace
            .version
            .as_ref()
            .unwrap()
    );

    Ok(())
}
