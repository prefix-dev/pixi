use clap::Parser;
use pixi_manifest::{FeatureName, PrioritizedChannel};
use rattler_conda_types::NamedChannelOrUrl;

use crate::{
    environment::{get_up_to_date_prefix, LockFileUsage},
    Project,
};

#[derive(Parser, Debug, Default)]
pub struct Args {
    /// The channel name(s) or URL
    #[clap(required = true, num_args=1..)]
    pub channel: Vec<NamedChannelOrUrl>,

    /// Don't update the environment, only remove the channel(s) from the
    /// lock-file.
    #[clap(long)]
    pub no_install: bool,

    /// The name of the feature to remove the channel from.
    #[clap(long, short)]
    pub feature: Option<String>,
}

pub async fn execute(mut project: Project, args: Args) -> miette::Result<()> {
    let feature_name = args
        .feature
        .map_or(FeatureName::Default, FeatureName::Named);

    // Remove the channels from the manifest
    project.manifest.remove_channels(
        args.channel.iter().cloned().map(PrioritizedChannel::from),
        &feature_name,
    )?;

    // Try to update the lock-file without the removed channels
    get_up_to_date_prefix(
        &project.default_environment(),
        LockFileUsage::Update,
        args.no_install,
    )
    .await?;
    project.save()?;

    // Report back to the user
    let channel_config = project.channel_config();
    for channel in args.channel {
        match channel {
            NamedChannelOrUrl::Name(ref name) => eprintln!(
                "{}Removed {} ({})",
                console::style(console::Emoji("✔ ", "")).green(),
                name,
                channel.clone().into_base_url(&channel_config)
            ),
            NamedChannelOrUrl::Url(url) => eprintln!(
                "{}Removed {}",
                console::style(console::Emoji("✔ ", "")).green(),
                url
            ),
        }
    }

    Ok(())
}
