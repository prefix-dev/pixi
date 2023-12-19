use crate::environment::update_prefix;
use crate::lock_file::{load_lock_file, update_lock_file};
use crate::prefix::Prefix;
use crate::Project;
use clap::Parser;
use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::{Channel, ChannelConfig, Platform};

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

    // Load the existing lock-file
    let lock_file = load_lock_file(&project).await?;

    // Remove the channels from the manifest
    project
        .manifest
        .remove_channels(channels_to_remove.iter().map(|(name, _channel)| name))?;

    // Try to update the lock-file with the new channels
    let lock_file = update_lock_file(&project, lock_file, None).await?;
    project.save()?;

    // Update the installation if needed
    if !args.no_install {
        // Get the currently installed packages
        let prefix = Prefix::new(project.environment_dir())?;
        let installed_packages = prefix.find_installed_packages(None).await?;

        // Update the prefix
        update_prefix(
            project.pypi_package_db()?,
            &prefix,
            installed_packages,
            &lock_file,
            Platform::current(),
        )
        .await?;
    }

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
