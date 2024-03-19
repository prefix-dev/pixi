use std::collections::HashMap;

use clap::Parser;
use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::{Channel, MatchSpec, ParseStrictness};

use crate::config::{Config, ConfigCli};

use super::{
    common::{find_installed_package, get_client_and_sparse_repodata, load_package_records},
    list::list_global_packages,
    upgrade::upgrade_package,
};

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
    let packages = list_global_packages().await?;
    let config = Config::with_cli_config(&args.config);
    let mut channels = config.compute_channels(&args.channel).into_diagnostic()?;

    let mut installed_versions = HashMap::with_capacity(packages.len());

    for package_name in packages.iter() {
        let prefix_record = find_installed_package(package_name).await?;
        let last_installed_channel = Channel::from_str(
            prefix_record.repodata_record.channel.clone(),
            config.channel_config(),
        )
        .into_diagnostic()?;

        channels.push(last_installed_channel);

        let installed_version = prefix_record
            .repodata_record
            .package_record
            .version
            .into_version();
        installed_versions.insert(package_name.as_normalized().to_owned(), installed_version);
    }

    // Remove possible duplicates
    channels = channels.into_iter().unique().collect::<Vec<_>>();

    // Fetch sparse repodata
    let (authenticated_client, sparse_repodata) =
        get_client_and_sparse_repodata(&channels, &config).await?;

    let mut upgraded = false;
    for package_name in packages.iter() {
        let package_matchspec =
            MatchSpec::from_str(package_name.as_source(), ParseStrictness::Strict)
                .into_diagnostic()?;
        let records = load_package_records(package_matchspec, &sparse_repodata)?;
        let package_record = records
            .iter()
            .find(|r| r.package_record.name.as_normalized() == package_name.as_normalized())
            .ok_or_else(|| {
                miette::miette!(
                    "Package {} not found in the specified channels",
                    package_name.as_normalized()
                )
            })?;
        let toinstall_version = package_record.package_record.version.version().to_owned();
        let installed_version = installed_versions
            .get(package_name.as_normalized())
            .expect("should have the installed version")
            .to_owned();

        // Prvent downgrades
        if toinstall_version.cmp(&installed_version) == std::cmp::Ordering::Greater {
            upgrade_package(
                package_name,
                installed_version,
                toinstall_version,
                records,
                authenticated_client.clone(),
            )
            .await?;
            upgraded = true;
        }
    }

    if !upgraded {
        eprintln!("Nothing to upgrade");
    }

    Ok(())
}
