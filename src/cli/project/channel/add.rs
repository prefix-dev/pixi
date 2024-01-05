use crate::environment::{get_up_to_date_prefix, LockFileUsage};
use crate::lock_file::load_lock_file;

use crate::Project;
use clap::Parser;
use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::{Channel, ChannelConfig};

#[derive(Parser, Debug, Default)]
pub struct Args {
    /// The channel name or URL
    #[clap(required = true, num_args=1..)]
    pub channel: Vec<String>,

    /// Don't update the environment, only add changed packages to the lock-file.
    #[clap(long)]
    pub no_install: bool,
}

pub async fn execute(mut project: Project, args: Args) -> miette::Result<()> {
    // Determine which channels are missing
    let channel_config = ChannelConfig::default();
    let channels = args
        .channel
        .into_iter()
        .map(|channel_str| {
            Channel::from_str(&channel_str, &channel_config).map(|channel| (channel_str, channel))
        })
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()?;

    let missing_channels = channels
        .into_iter()
        .filter(|(_name, channel)| !project.channels().contains(channel))
        .collect_vec();

    if missing_channels.is_empty() {
        eprintln!(
            "{}All channel(s) have already been added.",
            console::style(console::Emoji("✔ ", "")).green(),
        );
        return Ok(());
    }

    // Load the existing lock-file
    let _lock_file = load_lock_file(&project).await?;

    // Add the channels to the lock-file
    project
        .manifest
        .add_channels(missing_channels.iter().map(|(name, _channel)| name))?;

    get_up_to_date_prefix(&project, LockFileUsage::Update, args.no_install, None).await?;
    project.save()?;
    // Report back to the user
    for (name, channel) in missing_channels {
        eprintln!(
            "{}Added {} ({})",
            console::style(console::Emoji("✔ ", "")).green(),
            name,
            channel.base_url()
        );
    }

    Ok(())
}
