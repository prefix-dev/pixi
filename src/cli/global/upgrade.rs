use std::str::FromStr;

use clap::Parser;
use miette::IntoDiagnostic;
use rattler_conda_types::{Channel, ChannelConfig, MatchSpec, Platform};
use rattler_repodata_gateway::sparse::SparseRepoData;

use crate::repodata::fetch_sparse_repodata;

use super::{
    install::{install_package, package_name},
    list::list_global_packages,
};

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
    // Figure out what channels we are using
    let channel_config = ChannelConfig::default();
    let channels = args
        .channel
        .iter()
        .map(|c| Channel::from_str(c, &channel_config))
        .collect::<Result<Vec<Channel>, _>>()
        .into_diagnostic()?;

    // Find the MatchSpec we want to install
    let package_matchspec = MatchSpec::from_str(&package).into_diagnostic()?;
    let package_name = package_name(&package_matchspec)?;

    // Return with error if this package is not globally installed.
    if !list_global_packages()
        .await?
        .iter()
        .any(|global_package| global_package.as_source() == package)
    {
        // TODO: Maybe this can be an error
        miette::bail!(
            "{} Package is not globally installed",
            console::style("!").yellow().bold()
        );
    }

    let platform = Platform::current();

    // Fetch sparse repodata
    let platform_sparse_repodata = fetch_sparse_repodata(&channels, &[platform]).await?;

    let available_packages = SparseRepoData::load_records_recursive(
        platform_sparse_repodata.iter(),
        vec![package_name.clone()],
        None,
    )
    .into_diagnostic()?;

    // Install the package
    let (package_record, _, _) = install_package(
        package_matchspec,
        available_packages,
        &channel_config,
        platform,
    )
    .await?;

    eprintln!(
        "Updated package {} to version {}",
        package_name.as_source(),
        package_record.repodata_record.package_record.version
    );

    Ok(())
}
