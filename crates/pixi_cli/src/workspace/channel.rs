use std::io::Write;

use clap::Parser;
use fancy_display::FancyDisplay;
use miette::IntoDiagnostic;
use pixi_api::workspace::ChannelOptions;
use pixi_config::ConfigCli;
use pixi_core::environment::LockFileUsage;
use pixi_manifest::{EnvironmentName, FeatureName};
use rattler_conda_types::NamedChannelOrUrl;

use crate::{
    cli_config::{LockFileUpdateConfig, NoInstallConfig, ScriptWorkspaceConfig},
    cli_interface::cli_workspace_context,
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
pub struct ChannelArgs {
    /// The channel name or URL
    #[clap(required = true, num_args=1..)]
    pub channel: Vec<NamedChannelOrUrl>,

    /// Specify the channel priority
    #[clap(long, num_args = 1)]
    pub priority: Option<i32>,

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
pub struct AddArgs {
    #[clap(flatten)]
    pub args: ChannelArgs,

    /// Deprecated: use `pixi workspace channel prepend` instead
    #[clap(long)]
    pub prepend: bool,
}

#[derive(Parser, Debug, Default, Clone)]
pub struct ListArgs {
    /// Whether to display the channel's names or urls
    #[clap(long)]
    pub urls: bool,
}

impl TryFrom<&ChannelArgs> for ChannelOptions {
    type Error = miette::Report;

    fn try_from(args: &ChannelArgs) -> Result<Self, Self::Error> {
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
    /// Adds channels to the manifest and updates the lock file.
    ///
    /// New channels are appended to the channel list. Channels that are
    /// already listed keep their position; `--priority` updates their
    /// priority and other existing settings are kept.
    #[clap(visible_alias = "a")]
    Add(AddArgs),
    /// List the channels in the manifest.
    #[clap(visible_alias = "ls")]
    List(ListArgs),
    /// Moves or adds channels to the front of the manifest's channel list and
    /// updates the lock file.
    ///
    /// The given channels end up first, in the given order. Channels that are
    /// already listed are moved instead of duplicated and keep their existing
    /// settings unless `--priority` overrides the priority. Channels with a
    /// higher numeric priority still take precedence over channels listed
    /// before them.
    Prepend(ChannelArgs),
    /// Remove channel(s) from the manifest and updates the lock file.
    #[clap(visible_alias = "rm")]
    Remove(ChannelArgs),
}

impl Args {
    fn validate_script_options(&self) -> miette::Result<()> {
        if self.workspace_config.script.is_none() {
            return Ok(());
        }

        let mutation_args = match &self.command {
            Command::Add(AddArgs { args, .. }) | Command::Prepend(args) | Command::Remove(args) => {
                args
            }
            Command::List(_) => return Ok(()),
        };

        let mut unsupported = Vec::new();
        if mutation_args.feature.is_some() {
            unsupported.push("--feature");
        }
        if mutation_args.environment.is_some() {
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
    if let Command::Add(AddArgs { prepend: true, .. }) = &args.command {
        eprintln!(
            "{}the '{}' option is deprecated; use '{}' instead",
            console::style(console::Emoji("⚠️ ", "")).yellow(),
            console::style("--prepend").bold().red(),
            console::style("pixi workspace channel prepend")
                .bold()
                .green(),
        );
    }
    args.validate_script_options()?;

    let workspace = crate::workspace_locator()
        .with_global_config_source(args.config_source.source())
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()
        .await?;

    let is_script = args.workspace_config.script.is_some();
    let lock_file_exists = workspace.lock_file_path().is_file();
    let channel_config = workspace.channel_config();
    let workspace_ctx = cli_workspace_context(workspace);

    match args.command {
        Command::Add(AddArgs { args, prepend }) => {
            let options = channel_options(&args, is_script, lock_file_exists)?;
            Ok(workspace_ctx
                .add_channel(options, args.priority, prepend)
                .await?)
        }
        Command::Prepend(args) => {
            let options = channel_options(&args, is_script, lock_file_exists)?;
            Ok(workspace_ctx
                .add_channel(options, args.priority, true)
                .await?)
        }
        Command::List(args) => {
            let environments = workspace_ctx.list_channel().await;
            for (env_name, channels) in environments {
                let _ = pixi_utils::io::ignore_broken_pipe(writeln!(
                    std::io::stdout(),
                    "{} {}",
                    console::style("Environment:").bold().bright(),
                    env_name.fancy_display()
                ));

                for channel in channels {
                    let _ = pixi_utils::io::ignore_broken_pipe(writeln!(
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
                    ));
                }
            }
            Ok(())
        }
        Command::Remove(args) => {
            let options = channel_options(&args, is_script, lock_file_exists)?;
            Ok(workspace_ctx.remove_channel(options, args.priority).await?)
        }
    }
}

fn channel_options(
    args: &ChannelArgs,
    is_script: bool,
    lock_file_exists: bool,
) -> miette::Result<ChannelOptions> {
    let mut options: ChannelOptions = args.try_into()?;
    options.lock_file_usage =
        channel_lock_file_usage(options.lock_file_usage, is_script, lock_file_exists);
    Ok(options)
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
