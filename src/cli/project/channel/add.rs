use clap::Parser;
use pixi_manifest::{FeatureName, PrioritizedChannel};
use rattler_conda_types::NamedChannelOrUrl;

use crate::{
    environment::{get_up_to_date_prefix, LockFileUsage},
    Project,
};
#[derive(Parser, Debug, Default)]
pub struct Args {
    /// The channel name or URL
    #[clap(required = true, num_args=1..)]
    pub channel: Vec<NamedChannelOrUrl>,

    /// Don't update the environment, only add changed packages to the
    /// lock-file.
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

    // Add the channels to the manifest
    project.manifest.add_channels(
        args.channel
            .clone()
            .into_iter()
            .map(|channel| PrioritizedChannel {
                channel,
                priority: None,
            }),
        &feature_name,
    )?;

    // TODO: Update all environments touched by the features defined.
    get_up_to_date_prefix(
        &project.default_environment(),
        LockFileUsage::Update,
        args.no_install,
    )
    .await?;
    project.save()?;
    // Report back to the user
    for channel in args.channel {
        match channel {
            NamedChannelOrUrl::Name(ref name) => eprintln!(
                "{}Added {} ({})",
                console::style(console::Emoji("✔ ", "")).green(),
                name,
                channel
                    .clone()
                    .into_base_url(project.config().channel_config())
            ),
            NamedChannelOrUrl::Url(url) => eprintln!(
                "{}Added {}",
                console::style(console::Emoji("✔ ", "")).green(),
                url
            ),
        }
    }

    Ok(())
}
