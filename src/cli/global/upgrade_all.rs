use clap::Parser;
use itertools::Itertools;

use rattler_conda_types::MatchSpec;

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
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);

    let names = list_global_packages().await?;
    let specs = names
        .iter()
        .map(|name| MatchSpec {
            name: Some(name.clone()),
            ..Default::default()
        })
        .collect_vec();

    upgrade_packages(names, specs, config, &args.channel).await
}
