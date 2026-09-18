use std::io::Write;

use clap::Parser;
use miette::IntoDiagnostic;
use pixi_core::Workspace;

#[derive(Parser, Debug, Default)]
pub struct Args {}

pub async fn execute(workspace: Workspace) -> miette::Result<()> {
    if !workspace.workspace.provenance.is_pyproject() {
        miette::bail!(
            "`requires-python` is only supported in `pyproject.toml` manifests. For `pixi.toml`, specify python under `[dependencies]`."
        );
    }

    if let Some(spec) = &workspace.workspace.value.workspace.requires_python {
        pixi_utils::io::ignore_broken_pipe(writeln!(std::io::stdout(), "{spec}"))
            .into_diagnostic()?;
    }

    Ok(())
}
