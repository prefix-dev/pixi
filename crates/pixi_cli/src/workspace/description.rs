use std::io::Write;

use clap::Parser;
use miette::IntoDiagnostic;
use pixi_api::WorkspaceContext;
use pixi_core::WorkspaceLocator;

use crate::{cli_config::WorkspaceConfig, cli_interface::CliInterface};

/// Commands to manage workspace description.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    /// The subcommand to execute
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug, Default)]
pub struct SetArgs {
    /// The workspace description
    #[clap(required = true, num_args = 1)]
    pub description: String,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Get the workspace description.
    ///
    /// Example:
    /// `pixi workspace description get`
    Get,
    /// Set the workspace description.
    ///
    /// Example:
    /// `pixi workspace description set "My awesome workspace"`
    Set(SetArgs),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    let workspace_ctx = WorkspaceContext::new(CliInterface {}, workspace);

    match args.command {
        Command::Get => {
            // Print the description if it exists
            if let Some(description) = workspace_ctx.description().await {
                writeln!(std::io::stdout(), "{description}")
                    .inspect_err(|e| {
                        if e.kind() == std::io::ErrorKind::BrokenPipe {
                            std::process::exit(0);
                        }
                    })
                    .into_diagnostic()?;
            }
        }
        Command::Set(args) => {
            workspace_ctx
                .set_description(Some(args.description))
                .await?
        }
    }

    Ok(())
}
