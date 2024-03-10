use clap::Parser;
use futures::{stream, StreamExt, TryStreamExt};
use miette::IntoDiagnostic;
use rattler_conda_types::{Channel, ChannelConfig, Platform};
use rattler_networking::AuthenticationMiddleware;
use std::collections::HashMap;
use std::sync::Arc;

use crate::prefix::Prefix;
use crate::repodata::fetch_sparse_repodata;

use super::install::{find_designated_package, globally_install_package, BinEnvDir};
use super::list::list_global_packages;

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

    // Map packages to current prefix record
    let mut pkg_to_orig_record = HashMap::new();
    for package_name in packages.iter() {
        let BinEnvDir(bin_env_prefix) = BinEnvDir::from_existing(package_name).await?;
        let prefix = Prefix::new(bin_env_prefix);

        // Find the installed package in the environment
        let prefix_package = find_designated_package(&prefix, package_name).await?;
        pkg_to_orig_record.insert(package_name.clone(), prefix_package);
    }

    let authenticated_client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
        .with_arc(Arc::new(AuthenticationMiddleware::default()))
        .build();
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
            let orig_record = pkg_to_orig_record
                .get(&record.name)
                .expect("global package should already exist")
                .repodata_record
                .package_record
                .clone();
            eprintln!(
                "Upgraded {}: {} {} -> {} {}",
                console::style(record.name.as_normalized()).bold(),
                console::style(orig_record.version).bold(),
                console::style(orig_record.build).bold(),
                console::style(record.version).bold(),
                console::style(record.build).bold()
            );
        }
    }

    if packages_upgraded == 0 {
        eprintln!("Nothing to upgrade");
    }

    Ok(())
}
