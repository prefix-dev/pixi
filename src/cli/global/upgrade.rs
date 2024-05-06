use std::collections::HashMap;
use std::time::Duration;

use clap::Parser;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use indexmap::IndexMap;
use indicatif::ProgressBar;
use itertools::Itertools;
use miette::{IntoDiagnostic, Report};
use rattler_conda_types::{Channel, MatchSpec, PackageName, Platform};

use crate::config::Config;
use crate::progress::{global_multi_progress, long_running_progress_style};

use super::common::{
    find_installed_package, get_client_and_sparse_repodata, load_package_records, HasSpecs,
};
use super::install::globally_install_package;

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

    /// The platform to install the package for.
    #[clap(long, default_value_t = Platform::current())]
    platform: Platform,
}

impl HasSpecs for Args {
    fn packages(&self) -> Vec<&str> {
        self.specs.iter().map(AsRef::as_ref).collect()
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::load_global();
    let specs = args.specs()?;
    upgrade_packages(specs, config, &args.channel, &args.platform).await
}

pub(super) async fn upgrade_packages(
    specs: IndexMap<PackageName, MatchSpec>,
    config: Config,
    cli_channels: &[String],
    platform: &Platform,
) -> miette::Result<()> {
    let channel_cli = config.compute_channels(cli_channels).into_diagnostic()?;

    // Get channels and version of globally installed packages in parallel
    let mut channels = HashMap::with_capacity(specs.len());
    let mut versions = HashMap::with_capacity(specs.len());
    let mut handles = FuturesUnordered::<tokio::task::JoinHandle<Result<_, Report>>>::new();
    for package_name in specs.keys().cloned() {
        let channel_config = config.channel_config().clone();
        let future: tokio::task::JoinHandle<Result<_, Report>> = tokio::spawn(async move {
            let p = find_installed_package(&package_name).await?;
            let channel =
                Channel::from_str(p.repodata_record.channel, &channel_config).into_diagnostic()?;
            let version = p.repodata_record.package_record.version.into_version();
            Ok((package_name, channel, version))
        });
        handles.push(future);
    }
    while let Some(data) = handles.next().await {
        let (package_name, channel, version) = data.into_diagnostic()??;
        channels.insert(package_name.clone(), channel);
        versions.insert(package_name, version);
    }

    // Fetch sparse repodata across all channels
    let all_channels = channels.values().chain(channel_cli.iter()).unique();
    let (client, repodata) =
        get_client_and_sparse_repodata(all_channels, *platform, &config).await?;

    // Upgrade each package when relevant
    let mut upgraded = false;
    for (package_name, package_matchspec) in specs.iter() {
        // Filter repodata based on channels specific to the package (and from the CLI)
        let specific_repodata = repodata.iter().filter_map(|((c, _), v)| {
            if channel_cli.contains(c) || channels.get(package_name).unwrap() == c {
                Some(v)
            } else {
                None
            }
        });
        let records = load_package_records(package_matchspec.clone(), specific_repodata)?;
        let package_record = records
            .iter()
            .find(|r| r.package_record.name == *package_name)
            .ok_or_else(|| {
                miette::miette!(
                    "Package {} not found in the specified channels",
                    package_name.as_normalized()
                )
            })?;
        let toinstall_version = package_record.package_record.version.version().to_owned();
        let installed_version = versions
            .get(package_name)
            .expect("should have the installed version")
            .to_owned();

        // Perform upgrade if a specific version was requested
        // OR if a more recent version is available
        if package_matchspec.version.is_some() || toinstall_version > installed_version {
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
            globally_install_package(package_name, records, client.clone(), platform).await?;
            pb.finish_with_message(format!("{} {}", console::style("Updated").green(), message));
            upgraded = true;
        }
    }

    if !upgraded {
        eprintln!("Nothing to upgrade");
    }

    Ok(())
}
