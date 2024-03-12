use crate::environment::{get_up_to_date_prefix, LockFileUsage};

use crate::project::manifest::channel::PrioritizedChannel;
use crate::project::manifest::FeatureName;
use crate::Project;
use clap::Parser;
use indexmap::IndexMap;
use miette::IntoDiagnostic;
use rattler_conda_types::Channel;

#[derive(Parser, Debug, Default)]
pub struct Args {
    /// The channel name(s) or URL
    #[clap(required = true, num_args=1..)]
    pub channel: Vec<String>,

    /// Don't update the environment, only remove the channel(s) from the lock-file.
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

    // Determine which channels to remove
    let channels = args
        .channel
        .into_iter()
        .map(|channel_str| {
            Channel::from_str(&channel_str, project.config().channel_config())
                .map(|channel| (channel_str, channel))
        })
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()?;

    // Remove the channels from the manifest
    project.manifest.remove_channels(
        channels
            .clone()
            .into_iter()
            .map(|(_name, channel)| channel)
            .map(PrioritizedChannel::from_channel),
        &feature_name,
    )?;

    // Try to update the lock-file without the removed channels
    get_up_to_date_prefix(
        &project.default_environment(),
        LockFileUsage::Update,
        args.no_install,
        IndexMap::default(),
    )
    .await?;
    project.save()?;

    // Report back to the user
    for (name, channel) in channels {
        eprintln!(
            "{}Removed {} ({})",
            console::style(console::Emoji("✔ ", "")).green(),
            name,
            channel.base_url()
        );
    }

    Ok(())
}
