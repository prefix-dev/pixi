use std::time::Duration;

use clap::Parser;
use indicatif::ProgressBar;
use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::{Channel, MatchSpec, PackageName, Platform, Version};
use rattler_conda_types::{ParseStrictness, RepoDataRecord};
use reqwest_middleware::ClientWithMiddleware;

use crate::config::Config;
use crate::progress::{global_multi_progress, long_running_progress_style};

use super::common::{
    find_installed_package, get_client_and_sparse_repodata, load_package_records, package_name,
};
use super::install::globally_install_package;
use super::list::list_global_packages;

/// Upgrade specific package which is installed globally.
#[derive(Parser, Debug)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    /// Specifies the package that is to be upgraded.
    package: String,

    /// Represents the channels from which to upgrade specified package.
    /// Multiple channels can be specified by using this field multiple times.
    ///
    /// When specifying a channel, it is common that the selected channel also
    /// depends on the `conda-forge` channel.
    /// For example: `pixi global upgrade --channel conda-forge --channel bioconda`.
    ///
    /// By default, if no channel is provided, `conda-forge` is used, the channel
    /// the package was installed from will always be used.
    #[clap(short, long)]
    channel: Vec<String>,

    /// The platform to install the package for.
    #[clap(long, default_value_t = Platform::current())]
    platform: Platform,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    // Get the MatchSpec we need to upgrade
    let package_matchspec =
        MatchSpec::from_str(&args.package, ParseStrictness::Strict).into_diagnostic()?;
    let package_name = package_name(&package_matchspec)?;
    let matchspec_has_version = package_matchspec.version.is_some();

    // Return with error if this package is not globally installed.
    if !list_global_packages()
        .await?
        .iter()
        .any(|global_package| global_package.as_normalized() == package_name.as_normalized())
    {
        miette::bail!(
            "Package {} is not globally installed",
            package_name.as_source()
        );
    }

    let prefix_record = find_installed_package(&package_name).await?;
    let installed_version = prefix_record
        .repodata_record
        .package_record
        .version
        .into_version();

    let config = Config::load_global();

    // Figure out what channels we are using
    let last_installed_channel = Channel::from_str(
        prefix_record.repodata_record.channel.clone(),
        config.channel_config(),
    )
    .into_diagnostic()?;

    let mut channels = vec![last_installed_channel];
    let input_channels = args
        .channel
        .iter()
        .map(|c| Channel::from_str(c, config.channel_config()))
        .collect::<Result<Vec<Channel>, _>>()
        .into_diagnostic()?;
    channels.extend(input_channels);
    // Remove possible duplicates
    channels = channels.into_iter().unique().collect::<Vec<_>>();

    // Fetch sparse repodata
    let (authenticated_client, sparse_repodata) =
        get_client_and_sparse_repodata(&channels, args.platform.clone(), &config).await?;

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

    if !matchspec_has_version
        && toinstall_version.cmp(&installed_version) != std::cmp::Ordering::Greater
    {
        eprintln!(
            "Package {} is already up-to-date",
            package_name.as_normalized(),
        );
        return Ok(());
    }

    upgrade_package(
        &package_name,
        installed_version,
        toinstall_version,
        records,
        authenticated_client,
        &args.platform,
    )
    .await
}

pub(super) async fn upgrade_package(
    package_name: &PackageName,
    installed_version: Version,
    toinstall_version: Version,
    records: Vec<RepoDataRecord>,
    authenticated_client: ClientWithMiddleware,
    platform: &Platform,
) -> miette::Result<()> {
    let message = format!(
        "{} v{} -> v{}",
        package_name.as_normalized(),
        installed_version,
        toinstall_version
    );

    let pb = global_multi_progress().add(ProgressBar::new_spinner());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_style(long_running_progress_style());
    pb.set_message(format!(
        "{} {}",
        console::style("Updating").green(),
        message
    ));
    globally_install_package(package_name, records, authenticated_client, platform).await?;
    pb.finish_with_message(format!("{} {}", console::style("Updated").green(), message));
    Ok(())
}
