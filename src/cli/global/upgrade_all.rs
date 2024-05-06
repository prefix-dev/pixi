use clap::Parser;
use indexmap::IndexMap;

use rattler_conda_types::{MatchSpec, Platform};

use crate::config::{Config, ConfigCli};

use super::{list::list_global_packages, upgrade::upgrade_packages};

/// Upgrade all globally installed packages
#[derive(Parser, Debug)]
pub struct Args {
    /// Represents the channels from which to upgrade packages.
    /// Multiple channels can be specified by using this field multiple times.
    ///
    /// When specifying a channel, it is common that the selected channel also
    /// depends on the `conda-forge` channel.
    /// For example: `pixi global upgrade-all --channel conda-forge --channel bioconda`.
    ///
    /// By default, if no channel is provided, `conda-forge` is used, the channel
    /// the package was installed from will always be used.
    #[clap(short, long)]
    channel: Vec<String>,

    #[clap(flatten)]
    config: ConfigCli,

    /// The platform to install the package for.
    #[clap(long, default_value_t = Platform::current())]
    platform: Platform,
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

    upgrade_packages(specs, config, &args.channel, &args.platform).await
}
