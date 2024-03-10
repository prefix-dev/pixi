use std::sync::Arc;

use clap::Parser;
use miette::IntoDiagnostic;
use rattler_conda_types::ParseStrictness::Strict;
use rattler_conda_types::{Channel, ChannelConfig, MatchSpec, Platform};
use rattler_networking::AuthenticationMiddleware;

use crate::prefix::Prefix;
use crate::repodata::fetch_sparse_repodata;

use super::install::{find_designated_package, globally_install_package, BinEnvDir};
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
    /// By default, if no channel is provided, `conda-forge` is used.
    #[clap(short, long, default_values = ["conda-forge"])]
    channel: Vec<String>,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let package = args.package;
    let all_packages = list_global_packages().await?;

    // Return with error if this package is not globally installed.
    let Some(package_name) = all_packages.iter().find(|p| p.as_source() == package) else {
        miette::bail!(
            "{} package is not globally installed",
            console::style("!").yellow().bold()
        );
    };

    // Find the current version of the package
    let BinEnvDir(bin_env_prefix) = BinEnvDir::from_existing(package_name).await?;
    let prefix = Prefix::new(bin_env_prefix);
    let old_prefix_package = find_designated_package(&prefix, package_name).await?;
    let old_package_record = old_prefix_package.repodata_record.package_record;

    // Figure out what channels we are using
    let channel_config = ChannelConfig::default();
    let channels = args
        .channel
        .iter()
        .map(|c| Channel::from_str(c, &channel_config))
        .collect::<Result<Vec<Channel>, _>>()
        .into_diagnostic()?;

    // Find the MatchSpec we want to install
    let package_matchspec = MatchSpec::from_str(&package, Strict).into_diagnostic()?;

    let authenticated_client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
        .with_arc(Arc::new(AuthenticationMiddleware::default()))
        .build();
    // Fetch sparse repodata
    let platform_sparse_repodata =
        fetch_sparse_repodata(&channels, [Platform::current()], &authenticated_client).await?;

    // Install the package
    let (package_record, _, upgraded) = globally_install_package(
        package_matchspec,
        &platform_sparse_repodata,
        &channel_config,
        authenticated_client,
    )
    .await?;

    let package_record = package_record.repodata_record.package_record;
    if upgraded {
        eprintln!(
            "Updated package {} version from {} to {}",
            package_record.name.as_normalized(),
            old_package_record.version,
            package_record.version
        );
    } else {
        eprintln!(
            "Package {} is already up-to-date",
            package_record.name.as_normalized(),
        );
    }

    Ok(())
}
