use std::io::Write;

use miette::IntoDiagnostic;
use pixi_core::Workspace;

pub async fn execute(workspace: Workspace) -> miette::Result<()> {
    // Print the description if it exists
    if let Some(description) = workspace.workspace.value.workspace.description {
        writeln!(std::io::stdout(), "{}", description)
            .inspect_err(|e| {
                if e.kind() == std::io::ErrorKind::BrokenPipe {
                    std::process::exit(0);
                }
            })
            .into_diagnostic()?;
    }
    Ok(())
}
