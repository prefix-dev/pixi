use crate::environment::{get_up_to_date_prefix, LockFileUsage};
use crate::project::manifest::channel::PrioritizedChannel;
use crate::project::manifest::FeatureName;
use crate::Project;
use clap::Parser;
use indexmap::IndexMap;
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

    /// The name of the feature to add the channel to.
    #[clap(long, short)]
    pub feature: Option<String>,
}

pub async fn execute(mut project: Project, args: Args) -> miette::Result<()> {
    let feature_name = args
        .feature
        .map_or(FeatureName::Default, FeatureName::Named);

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

    // Add the channels to the manifest
    project.manifest.add_channels(
        channels
            .clone()
            .into_iter()
            .map(|(_name, channel)| channel)
            .map(PrioritizedChannel::from_channel),
        &feature_name,
    )?;

    // TODO: Update all environments touched by the features defined.
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
            "{}Added {} ({})",
            console::style(console::Emoji("✔ ", "")).green(),
            name,
            channel.base_url()
        );
    }

    Ok(())
}
