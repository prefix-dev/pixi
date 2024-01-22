use clap::Parser;
use futures::{stream, StreamExt, TryStreamExt};
use miette::IntoDiagnostic;
use rattler_conda_types::{Channel, ChannelConfig, Platform};
use rattler_networking::AuthenticatedClient;
use reqwest::Client;

use crate::repodata::fetch_sparse_repodata;

use super::{install::globally_install_package, list::list_global_packages};

const UPGRADE_ALL_CONCURRENCY: usize = 5;

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
    /// By default, if no channel is provided, `conda-forge` is used.
    #[clap(short, long, default_values = ["conda-forge"])]
    channel: Vec<String>,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    // Figure out what channels we are using
    let channel_config = ChannelConfig::default();
    let channels = args
        .channel
        .iter()
        .map(|c| Channel::from_str(c, &channel_config))
        .collect::<Result<Vec<Channel>, _>>()
        .into_diagnostic()?;

    let packages = list_global_packages().await?;

    let authenticated_client = AuthenticatedClient::from_client(Client::new(), Default::default());
    // Fetch sparse repodata
    let platform_sparse_repodata =
        fetch_sparse_repodata(&channels, [Platform::current()], &authenticated_client).await?;

    let tasks = packages
        .iter()
        .map(|package| package.as_source().parse())
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()?;

    let task_stream = stream::iter(tasks)
        .map(|matchspec| {
            globally_install_package(
                matchspec,
                &platform_sparse_repodata,
                &channel_config,
                authenticated_client.clone(),
            )
        })
        .buffered(UPGRADE_ALL_CONCURRENCY);

    let res: Vec<_> = task_stream.try_collect().await?;

    let mut packages_upgraded = 0;
    for (prefix_record, _, upgraded) in res {
        if upgraded {
            packages_upgraded += 1;
            let record = prefix_record.repodata_record.package_record;
            eprintln!(
                "Upgraded {} {}",
                console::style(record.name.as_normalized()).bold(),
                console::style(record.version).bold(),
            );
        }
    }

    if packages_upgraded == 0 {
        eprintln!("Nothing to upgrade");
    }

    Ok(())
}
