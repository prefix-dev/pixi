use clap::Parser;
use miette::IntoDiagnostic;

use fancy_display::FancyDisplay;
use pixi_core::WorkspaceLocator;
use pixi_manifest::FeaturesExt;

use crate::cli_config::WorkspaceConfig;

#[derive(Parser, Debug, Default, Clone)]
pub struct Args {
    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,
    /// Whether to display the channel's names or urls
    #[clap(long)]
    pub urls: bool,
}

pub(crate) fn execute(args: Args) -> miette::Result<()> {
    // Workspace without cli config as it shouldn't be needed here.
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    let channel_config = workspace.channel_config();
    workspace
        .environments()
        .iter()
        .map(|e| {
            println!(
                "{} {}",
                console::style("Environment:").bold().bright(),
                e.name().fancy_display()
            );
            e.channels()
        })
        .try_for_each(|c| -> Result<(), rattler_conda_types::ParseChannelError> {
            c.into_iter().try_for_each(
                |channel| -> Result<(), rattler_conda_types::ParseChannelError> {
                    println!(
                        "- {}",
                        if args.urls {
                            match channel.clone().into_base_url(&channel_config) {
                                Ok(url) => url.to_string(),
                                Err(e) => return Err(e),
                            }
                        } else {
                            channel.to_string()
                        }
                    );
                    Ok(())
                },
            )
        })
        .into_diagnostic()?;
    Ok(())
}
