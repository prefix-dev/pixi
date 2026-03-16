use std::io::Write;

use clap::Parser;
use miette::IntoDiagnostic;
use pixi_api::WorkspaceContext;
use pixi_core::WorkspaceLocator;

use crate::{cli_config::WorkspaceConfig, cli_interface::CliInterface};

/// Commands to manage workspace name.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    /// The subcommand to execute
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug)]
pub struct SetArgs {
    /// The workspace name, please only use lowercase letters (a-z), digits (0-9), hyphens (-), and underscores (_)
    #[clap(required = true, num_args = 1)]
    pub name: String,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Get the workspace name.
    Get,
    /// Set the workspace name.
    ///
    /// Example:
    /// `pixi workspace name set "my-workspace"`
    Set(SetArgs),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    let workspace_ctx = WorkspaceContext::new(CliInterface {}, workspace);

    match args.command {
        Command::Get => writeln!(std::io::stdout(), "{}", workspace_ctx.name().await)
            .inspect_err(|e| {
                if e.kind() == std::io::ErrorKind::BrokenPipe {
                    std::process::exit(0);
                }
            })
            .into_diagnostic()?,
        Command::Set(args) => workspace_ctx.set_name(&args.name).await?,
    }

    Ok(())
}
