use std::str::FromStr;
use clap::Parser;
use rattler_conda_types::{Channel, ChannelConfig, Platform, VersionSpec};
use reqwest::Client;
use crate::repodata::{fetch_sparse_repo_data, friendly_channel_name};

/// Runs command in project.
#[derive(Parser, Debug)]
#[clap(trailing_var_arg = true)]
pub struct Args {
    /// Package to install
    package: String,

    /// Channel to install from
    #[clap(short, long, default_values = ["conda-forge"])]
    channels: Vec<String>,
}


pub async fn execute(args: Args) -> anyhow::Result<()> {
    let channels = args.channels.iter().map(|c| Channel::from_str(c, &ChannelConfig::default())).collect::<Result<Vec<Channel>, _>>()?;
    let package = VersionSpec::from_str(&args.package).unwrap();
    let platform = Platform::current();

    println!("Installing: {}, from {}", package, channels.iter().map(|c| friendly_channel_name(c)).collect::<Vec<_>>().join(", "));

    fetch_sparse_repo_data(&channels, &vec![platform]).await?;


    Ok(())

}
