pub mod add;
pub mod list;
pub mod remove;

use crate::Project;
use clap::Parser;
use miette::IntoDiagnostic;
use pixi_manifest::{FeatureName, PrioritizedChannel};
use rattler_conda_types::{ChannelConfig, NamedChannelOrUrl};
use std::path::PathBuf;

/// Commands to manage project channels.
#[derive(Parser, Debug)]
pub struct Args {
    /// The path to `pixi.toml` or `pyproject.toml`
    #[clap(long, global = true)]
    pub manifest_path: Option<PathBuf>,

    /// The subcommand to execute
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug, Default)]
pub struct AddRemoveArgs {
    /// The channel name or URL
    #[clap(required = true, num_args=1..)]
    pub channel: Vec<NamedChannelOrUrl>,

    /// Specify the channel priority
    #[clap(long, num_args = 1)]
    pub priority: Option<i32>,

    /// Don't update the environment, only modify the manifest and the
    /// lock-file.
    #[clap(long)]
    pub no_install: bool,

    /// The name of the feature to modify.
    #[clap(long, short)]
    pub feature: Option<String>,
}

impl AddRemoveArgs {
    fn prioritized_channels(&self) -> impl IntoIterator<Item = PrioritizedChannel> + '_ {
        self.channel
            .iter()
            .cloned()
            .map(|channel| PrioritizedChannel::from((channel, self.priority)))
    }

    fn feature_name(&self) -> FeatureName {
        self.feature
            .clone()
            .map_or(FeatureName::Default, FeatureName::Named)
    }

    fn report(self, operation: &str, channel_config: &ChannelConfig) -> miette::Result<()> {
        for channel in self.channel {
            match channel {
                NamedChannelOrUrl::Name(ref name) => eprintln!(
                    "{}{operation} {} ({}){}",
                    console::style(console::Emoji("✔ ", "")).green(),
                    name,
                    channel
                        .clone()
                        .into_base_url(channel_config)
                        .into_diagnostic()?,
                    self.priority
                        .map_or_else(|| "".to_string(), |p| format!(" at priority {}", p))
                ),
                NamedChannelOrUrl::Url(url) => eprintln!(
                    "{}{operation} {}{}",
                    console::style(console::Emoji("✔ ", "")).green(),
                    url,
                    self.priority
                        .map_or_else(|| "".to_string(), |p| format!(" at priority {}", p)),
                ),
                NamedChannelOrUrl::Path(path) => eprintln!(
                    "{}{operation} {}",
                    console::style(console::Emoji("✔ ", "")).green(),
                    path
                ),
            }
        }
        Ok(())
    }
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Adds a channel to the project file and updates the lockfile.
    #[clap(visible_alias = "a")]
    Add(AddRemoveArgs),
    /// List the channels in the project file.
    #[clap(visible_alias = "ls")]
    List(list::Args),
    /// Remove channel(s) from the project file and updates the lockfile.
    #[clap(visible_alias = "rm")]
    Remove(AddRemoveArgs),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())?;

    match args.command {
        Command::Add(args) => add::execute(project, args).await,
        Command::List(args) => list::execute(project, args),
        Command::Remove(args) => remove::execute(project, args).await,
    }
}
