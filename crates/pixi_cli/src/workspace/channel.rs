use std::io::Write;

use clap::Parser;
use fancy_display::FancyDisplay;
use miette::IntoDiagnostic;
use pixi_api::{WorkspaceContext, workspace::ChannelOptions};
use pixi_config::ConfigCli;
use pixi_core::{WorkspaceLocator, environment::LockFileUsage};
use pixi_manifest::{EnvironmentName, FeatureName};
use rattler_conda_types::NamedChannelOrUrl;

use crate::{
    cli_config::{LockFileUpdateConfig, NoInstallConfig, ScriptWorkspaceConfig},
    cli_interface::CliInterface,
};

/// Commands to manage workspace channels.
#[derive(Parser, Debug, Clone)]
pub struct Args {
    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,

    #[clap(flatten)]
    pub workspace_config: ScriptWorkspaceConfig,

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
    pub feature: Option<FeatureName>,

    /// The environment to modify. The channel is written to the channels
    /// defined inline on the environment.
    #[clap(long, short, conflicts_with = "feature")]
    pub environment: Option<EnvironmentName>,
}

#[derive(Parser, Debug, Default, Clone)]
pub struct ListArgs {
    /// Whether to display the channel's names or urls
    #[clap(long)]
    pub urls: bool,
}

impl TryFrom<&AddRemoveArgs> for ChannelOptions {
    type Error = miette::Report;

    fn try_from(args: &AddRemoveArgs) -> Result<Self, Self::Error> {
        Ok(Self {
            channels: args.channel.clone(),
            feature: crate::cli_config::feature_from_flags(
                args.environment.as_ref(),
                args.feature.as_ref(),
            ),
            no_install: args.no_install_config.no_install,
            lock_file_usage: args.lock_file_update_config.lock_file_usage()?,
        })
    }
}

#[derive(Parser, Debug, Clone)]
pub enum Command {
    /// Adds a channel to the manifest and updates the lock file.
    #[clap(visible_alias = "a")]
    Add(AddRemoveArgs),
    /// List the channels in the manifest.
    #[clap(visible_alias = "ls")]
    List(ListArgs),
    /// Remove channel(s) from the manifest and updates the lock file.
    #[clap(visible_alias = "rm")]
    Remove(AddRemoveArgs),
}

impl Args {
    fn validate_script_options(&self) -> miette::Result<()> {
        if self.workspace_config.script.is_none() {
            return Ok(());
        }

        let add_remove = match &self.command {
            Command::Add(args) | Command::Remove(args) => args,
            Command::List(_) => return Ok(()),
        };

        let mut unsupported = Vec::new();
        if add_remove.feature.is_some() {
            unsupported.push("--feature");
        }
        if add_remove.environment.is_some() {
            unsupported.push("--environment");
        }

        if unsupported.is_empty() {
            Ok(())
        } else {
            Err(miette::miette!(
                help = "A PEP 723 script has one implicit default run environment.",
                "`pixi workspace channel --script` does not support {}",
                unsupported.join(", ")
            ))
        }
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    args.validate_script_options()?;

    let workspace = WorkspaceLocator::for_cli()
        .with_deprecation_warnings(true)
        .with_global_config_source(args.config_source.source())
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    let is_script = args.workspace_config.script.is_some();
    let lock_file_exists = workspace.lock_file_path().is_file();
    let channel_config = workspace.channel_config();
    let workspace_ctx = WorkspaceContext::new(CliInterface {}, workspace);

    match args.command {
        Command::Add(add_args) => {
            let priority = add_args.priority;
            let prepend = add_args.prepend;
            let mut options: ChannelOptions = (&add_args).try_into()?;
            options.lock_file_usage =
                channel_lock_file_usage(options.lock_file_usage, is_script, lock_file_exists);
            workspace_ctx.add_channel(options, priority, prepend).await
        }
        Command::List(args) => {
            let environments = workspace_ctx.list_channel().await;
            for (env_name, channels) in environments {
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

                for channel in channels {
                    let _ = writeln!(
                        std::io::stdout(),
                        "- {}",
                        if args.urls {
                            channel
                                .clone()
                                .into_base_url(&channel_config)
                                .into_diagnostic()?
                                .to_string()
                        } else {
                            channel.to_string()
                        }
                    )
                    .inspect_err(|e| {
                        if e.kind() == std::io::ErrorKind::BrokenPipe {
                            std::process::exit(0);
                        }
                    });
                }
            }
            Ok(())
        }
        Command::Remove(remove_args) => {
            let priority = remove_args.priority;
            let mut options: ChannelOptions = (&remove_args).try_into()?;
            options.lock_file_usage =
                channel_lock_file_usage(options.lock_file_usage, is_script, lock_file_exists);
            workspace_ctx.remove_channel(options, priority).await
        }
    }
}

fn channel_lock_file_usage(
    requested: LockFileUsage,
    is_script: bool,
    lock_file_exists: bool,
) -> LockFileUsage {
    if is_script && !lock_file_exists && requested == LockFileUsage::Update {
        LockFileUsage::Frozen
    } else {
        requested
    }
}
