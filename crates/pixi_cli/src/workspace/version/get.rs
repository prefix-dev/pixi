use std::io::Write;

use clap::Parser;
use miette::IntoDiagnostic;
use pixi_core::Workspace;

#[derive(Parser, Debug, Default)]
pub struct Args {}

pub async fn execute(workspace: Workspace, _args: Args) -> miette::Result<()> {
    // Print the version if it exists
    if let Some(version) = workspace.workspace.value.workspace.version {
        writeln!(std::io::stdout(), "{}", version)
            .inspect_err(|e| {
                if e.kind() == std::io::ErrorKind::BrokenPipe {
                    std::process::exit(0);
                }
            })
            .into_diagnostic()?;
    }
    Ok(())
}
