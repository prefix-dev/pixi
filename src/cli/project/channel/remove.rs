use crate::environment::{get_up_to_date_prefix, LockFileUsage};

use crate::Project;
use clap::Parser;
use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::{Channel, ChannelConfig};

#[derive(Parser, Debug, Default)]
pub struct Args {
    /// The channel name(s) or URL
    #[clap(required = true, num_args=1..)]
    pub channel: Vec<String>,

    /// Don't update the environment, only remove the channel(s) from the lock-file.
    #[clap(long)]
    pub no_install: bool,
}

pub async fn execute(mut project: Project, args: Args) -> miette::Result<()> {
    // Determine which channels to remove
    let channel_config = ChannelConfig::default();
    let channels = args
        .channel
        .into_iter()
        .map(|channel_str| {
            Channel::from_str(&channel_str, &channel_config).map(|channel| (channel_str, channel))
        })
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()?;

    let channels_to_remove = channels
        .into_iter()
        .filter(|(_name, channel)| project.channels().contains(channel))
        .collect_vec();

    if channels_to_remove.is_empty() {
        eprintln!(
            "{}The channel(s) are not present.",
            console::style(console::Emoji("✔ ", "")).green(),
        );
        return Ok(());
    }

    // Remove the channels from the manifest
    project
        .manifest
        .remove_channels(channels_to_remove.iter().map(|(name, _channel)| name))?;

    // Try to update the lock-file without the removed channels
    get_up_to_date_prefix(&project, LockFileUsage::Update, args.no_install, None).await?;
    project.save()?;

    // Report back to the user
    for (name, channel) in channels_to_remove {
        eprintln!(
            "{}Removed {} ({})",
            console::style(console::Emoji("✔ ", "")).green(),
            name,
            channel.base_url()
        );
    }

    Ok(())
}
