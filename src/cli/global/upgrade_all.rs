use clap::Parser;
use indexmap::IndexMap;

use rattler_conda_types::{MatchSpec, Platform};

use pixi_config::{Config, ConfigCli};

use crate::cli::cli_config::ChannelsConfig;

use super::{list::list_global_packages, upgrade::upgrade_packages};

/// Upgrade all globally installed packages
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    channels: ChannelsConfig,

    #[clap(flatten)]
    config: ConfigCli,

    /// The platform to install the package for.
    #[clap(long, default_value_t = Platform::current())]
    platform: Platform,

    /// Do not insert conda_prefix, path modifications, and activation script into the installed executable script.
    #[clap(long)]
    no_activation: bool,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);

    let names = list_global_packages().await?;
    let mut specs = IndexMap::with_capacity(names.len());
    for name in names {
        specs.insert(
            name.clone(),
            MatchSpec {
                name: Some(name),
                ..Default::default()
            },
        );
    }

    upgrade_packages(
        specs,
        config,
        args.channels,
        args.platform,
        args.no_activation,
    )
    .await
}
