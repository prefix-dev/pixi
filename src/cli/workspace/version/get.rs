use pixi_core::Workspace;
use clap::Parser;

#[derive(Parser, Debug, Default)]
pub struct Args {}

pub async fn execute(workspace: Workspace, _args: Args) -> miette::Result<()> {
    // Print the version if it exists
    if let Some(version) = workspace.workspace.value.workspace.version {
        println!("{}", version);
    }
    Ok(())
}
