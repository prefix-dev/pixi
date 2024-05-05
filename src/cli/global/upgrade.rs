use std::collections::{HashMap, HashSet};
use std::time::Duration;

use clap::Parser;
use indicatif::ProgressBar;
use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::{Channel, MatchSpec, PackageName, Version};
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
    /// Specifies the packages to upgrade.
    #[arg(required = true)]
    pub specs: Vec<String>,

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
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::load_global();

    // Get the MatchSpec(s) we need to upgrade
    let specs = args
        .specs
        .iter()
        .map(|p| MatchSpec::from_str(p, ParseStrictness::Strict).into_diagnostic())
        .collect::<Result<Vec<_>, _>>()?;
    let names = specs
        .iter()
        .map(package_name)
        .collect::<Result<Vec<_>, _>>()?;

    // Return with error if any package is not globally installed.
    let global_packages = list_global_packages()
        .await?
        .into_iter()
        .collect::<HashSet<_>>();
    let requested = names.iter().cloned().collect::<HashSet<_>>();
    let not_installed = requested.difference(&global_packages).collect_vec();
    match not_installed.len() {
        0 => {} // Do nothing when all packages are globally installed
        1 => miette::bail!(
            "Package {} is not globally installed",
            not_installed[0].as_normalized(),
        ),
        _ => miette::bail!(
            "Packages {} are not globally installed",
            not_installed.iter().map(|p| p.as_normalized()).join(", "),
        ),
    };

    upgrade_packages(names, specs, config, &args.channel).await
}

pub(super) async fn upgrade_package(
    package_name: &PackageName,
    installed_version: Version,
    toinstall_version: Version,
    records: Vec<RepoDataRecord>,
    authenticated_client: ClientWithMiddleware,
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
    globally_install_package(package_name, records, authenticated_client).await?;
    pb.finish_with_message(format!("{} {}", console::style("Updated").green(), message));
    Ok(())
}

pub(super) async fn upgrade_packages(
    names: Vec<PackageName>,
    specs: Vec<MatchSpec>,
    config: Config,
    cli_channels: &[String],
) -> Result<(), miette::Error> {
    // Get channels and versions of globally installed packages
    let mut installed_versions = HashMap::with_capacity(names.len());
    let mut channels = config.compute_channels(cli_channels).into_diagnostic()?;

    for package_name in names.iter() {
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
        installed_versions.insert(package_name.clone(), installed_version);
    }
    channels = channels.into_iter().unique().collect();

    // Fetch sparse repodata
    let (authenticated_client, sparse_repodata) =
        get_client_and_sparse_repodata(&channels, &config).await?;

    // Upgrade each package when relevant
    let mut upgraded = false;
    for (package_name, package_matchspec) in names.into_iter().zip(specs.into_iter()) {
        let matchspec_has_version = package_matchspec.version.is_some();
        let records = load_package_records(package_matchspec, &sparse_repodata)?;
        let package_record = records
            .iter()
            .find(|r| r.package_record.name == package_name)
            .ok_or_else(|| {
                miette::miette!(
                    "Package {} not found in the specified channels",
                    package_name.as_normalized()
                )
            })?;
        let toinstall_version = package_record.package_record.version.version().to_owned();
        let installed_version = installed_versions
            .get(&package_name)
            .expect("should have the installed version")
            .to_owned();

        // Perform upgrade if a specific version was requested
        // OR if a more recent version is available
        if matchspec_has_version || toinstall_version > installed_version {
            upgrade_package(
                &package_name,
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
