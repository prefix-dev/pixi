use clap::Parser;
use pixi_api::WorkspaceContext;
use pixi_core::WorkspaceLocator;
use pixi_manifest::FeatureName;

use crate::{cli_config::WorkspaceConfig, cli_interface::CliInterface};

/// Commands to manage workspace features.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    /// The subcommand to execute
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug)]
pub struct RemoveArgs {
    /// The name of the feature to remove
    pub feature: FeatureName,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Remove a feature from the manifest file.
    #[clap(visible_alias = "rm")]
    Remove(RemoveArgs),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    let workspace_ctx = WorkspaceContext::new(CliInterface {}, workspace);

    match args.command {
        Command::Remove(args) => {
            workspace_ctx.remove_feature(&args.feature).await?;
        }
    }

    Ok(())
}
