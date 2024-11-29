pub mod add;
pub mod list;
pub mod remove;

use crate::cli::cli_config::{PrefixUpdateConfig, ProjectConfig};
use clap::Parser;
use miette::IntoDiagnostic;
use pixi_manifest::{FeatureName, PrioritizedChannel};
use rattler_conda_types::{ChannelConfig, NamedChannelOrUrl};

/// Commands to manage project channels.
#[derive(Parser, Debug, Clone)]
pub struct Args {
    /// The subcommand to execute
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug, Default, Clone)]
pub struct AddRemoveArgs {
    #[clap(flatten)]
    pub project_config: ProjectConfig,
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
    pub prefix_update_config: PrefixUpdateConfig,

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

#[derive(Parser, Debug, Clone)]
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
    match args.command {
        Command::Add(add_args) => add::execute(add_args).await,
        Command::List(args) => list::execute(args),
        Command::Remove(remove_args) => remove::execute(remove_args).await,
    }
}
