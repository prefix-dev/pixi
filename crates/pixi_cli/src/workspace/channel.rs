use std::io::Write;

use clap::Parser;
use fancy_display::FancyDisplay;
use miette::IntoDiagnostic;
use pixi_api::{WorkspaceContext, workspace::ChannelOptions};
use pixi_config::ConfigCli;
use pixi_core::WorkspaceLocator;
use rattler_conda_types::NamedChannelOrUrl;

use crate::{
    cli_config::{LockFileUpdateConfig, NoInstallConfig, WorkspaceConfig},
    cli_interface::CliInterface,
};

/// Commands to manage workspace channels.
#[derive(Parser, Debug, Clone)]
pub struct Args {
    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    /// The subcommand to execute
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug, Default, Clone)]
pub struct AddRemoveArgs {
    /// The channel name or URL
    #[clap(required = true, num_args=1..)]
    pub channel: Vec<NamedChannelOrUrl>,

    /// Specify the channel priority
    #[clap(long, num_args = 1)]
    pub priority: Option<i32>,

    /// Add the channel(s) to the beginning of the channels list, making them the highest priority
    #[clap(long)]
    pub prepend: bool,

    #[clap(flatten)]
    pub no_install_config: NoInstallConfig,
    #[clap(flatten)]
    pub lock_file_update_config: LockFileUpdateConfig,

    #[clap(flatten)]
    pub config: ConfigCli,

    /// The name of the feature to modify.
    #[clap(long, short)]
    pub feature: Option<String>,
}

#[derive(Parser, Debug, Default, Clone)]
pub struct ListArgs {
    /// Whether to display the channel's names or urls
    #[clap(long)]
    pub urls: bool,
}

impl TryFrom<AddRemoveArgs> for ChannelOptions {
    type Error = miette::Report;

    fn try_from(args: AddRemoveArgs) -> Result<Self, Self::Error> {
        Ok(Self {
            channel: args.channel,
            priority: args.priority,
            prepend: args.prepend,
            feature: args.feature,
            no_install: args.no_install_config.no_install,
            lock_file_usage: args.lock_file_update_config.lock_file_usage()?,
        })
    }
}

#[derive(Parser, Debug, Clone)]
pub enum Command {
    /// Adds a channel to the manifest and updates the lockfile.
    #[clap(visible_alias = "a")]
    Add(AddRemoveArgs),
    /// List the channels in the manifest.
    #[clap(visible_alias = "ls")]
    List(ListArgs),
    /// Remove channel(s) from the manifest and updates the lockfile.
    #[clap(visible_alias = "rm")]
    Remove(AddRemoveArgs),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    let channel_config = workspace.channel_config();
    let workspace_ctx = WorkspaceContext::new(CliInterface {}, workspace);

    match args.command {
        Command::Add(add_args) => workspace_ctx.add_channel(add_args.try_into()?).await,
        Command::List(args) => workspace_ctx
            .list_channel()
            .await
            .into_iter()
            .map(|(env_name, channels)| {
                let _ = writeln!(
                    std::io::stdout(),
                    "{} {}",
                    console::style("Environment:").bold().bright(),
                    env_name.fancy_display()
                )
                .inspect_err(|e| {
                    if e.kind() == std::io::ErrorKind::BrokenPipe {
                        std::process::exit(0);
                    }
                });
                channels
            })
            .try_for_each(|c| -> Result<(), rattler_conda_types::ParseChannelError> {
                c.into_iter().try_for_each(
                    |channel| -> Result<(), rattler_conda_types::ParseChannelError> {
                        let _ = writeln!(
                            std::io::stdout(),
                            "- {}",
                            if args.urls {
                                match channel.clone().into_base_url(&channel_config) {
                                    Ok(url) => url.to_string(),
                                    Err(e) => return Err(e),
                                }
                            } else {
                                channel.to_string()
                            }
                        )
                        .inspect_err(|e| {
                            if e.kind() == std::io::ErrorKind::BrokenPipe {
                                std::process::exit(0);
                            }
                        });
                        Ok(())
                    },
                )
            })
            .into_diagnostic(),
        Command::Remove(remove_args) => workspace_ctx.remove_channel(remove_args.try_into()?).await,
    }
}
