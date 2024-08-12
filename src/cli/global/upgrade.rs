use std::{collections::HashMap, sync::Arc, time::Duration};

use clap::Parser;
use indexmap::IndexMap;
use indicatif::ProgressBar;
use itertools::Itertools;
use miette::{Context, IntoDiagnostic, Report};
use pixi_utils::reqwest::build_reqwest_clients;
use rattler_conda_types::{Channel, GenericVirtualPackage, MatchSpec, PackageName, Platform};
use rattler_solve::{resolvo::Solver, SolverImpl, SolverTask};
use rattler_virtual_packages::VirtualPackage;
use tokio::task::JoinSet;

use super::{common::find_installed_package, install::globally_install_package};
use crate::cli::{cli_config::ChannelsConfig, has_specs::HasSpecs};
use pixi_config::Config;
use pixi_progress::{global_multi_progress, long_running_progress_style, wrap_in_progress};

/// Upgrade specific package which is installed globally.
#[derive(Parser, Debug)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    /// Specifies the packages to upgrade.
    #[arg(required = true)]
    pub specs: Vec<String>,

    #[clap(flatten)]
    channels: ChannelsConfig,

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
    upgrade_packages(specs, config, &args.channels, args.platform).await
}

pub(super) async fn upgrade_packages(
    specs: IndexMap<PackageName, MatchSpec>,
    config: Config,
    cli_channels: &ChannelsConfig,
    platform: Platform,
) -> miette::Result<()> {
    let channel_cli = cli_channels.resolve_from_config(&config);

    // Get channels and version of globally installed packages in parallel
    let mut channels = HashMap::with_capacity(specs.len());
    let mut versions = HashMap::with_capacity(specs.len());
    let mut set: JoinSet<Result<_, Report>> = JoinSet::new();
    for package_name in specs.keys().cloned() {
        let channel_config = config.global_channel_config().clone();
        set.spawn(async move {
            let p = find_installed_package(&package_name).await?;
            let channel =
                Channel::from_str(p.repodata_record.channel, &channel_config).into_diagnostic()?;
            let version = p.repodata_record.package_record.version.into_version();
            Ok((package_name, channel, version))
        });
    }
    while let Some(data) = set.join_next().await {
        let (package_name, channel, version) = data.into_diagnostic()??;
        channels.insert(package_name.clone(), channel);
        versions.insert(package_name, version);
    }

    // Fetch repodata across all channels

    // Start by aggregating all channels that we need to iterate
    let all_channels: Vec<Channel> = channels
        .values()
        .cloned()
        .chain(channel_cli.iter().cloned())
        .unique()
        .collect();

    // Now ask gateway to query repodata for these channels
    let (_, authenticated_client) = build_reqwest_clients(Some(&config));
    let gateway = config.gateway(authenticated_client.clone());
    let repodata = gateway
        .query(
            all_channels,
            [platform, Platform::NoArch],
            specs.values().cloned().collect_vec(),
        )
        .recursive(true)
        .await
        .into_diagnostic()?;

    // Resolve environments in parallel
    let mut set: JoinSet<Result<_, Report>> = JoinSet::new();

    // Create arcs for these structs
    // as they later will be captured by closure
    let repodata = Arc::new(repodata);
    let config = Arc::new(config);
    let channel_cli = Arc::new(channel_cli);
    let channels = Arc::new(channels);

    for (package_name, package_matchspec) in specs {
        let repodata = repodata.clone();
        let config = config.clone();
        let channel_cli = channel_cli.clone();
        let channels = channels.clone();

        set.spawn_blocking(move || {
            // Filter repodata based on channels specific to the package (and from the CLI)
            let specific_repodata: Vec<_> = repodata
                .iter()
                .filter_map(|repodata| {
                    let filtered: Vec<_> = repodata
                        .iter()
                        .filter(|item| {
                            let item_channel =
                                Channel::from_str(&item.channel, config.global_channel_config())
                                    .expect("should be parseable");
                            channel_cli.contains(&item_channel)
                                || channels
                                    .get(&package_name)
                                    .map_or(false, |c| c == &item_channel)
                        })
                        .collect();

                    (!filtered.is_empty()).then_some(filtered)
                })
                .collect();

            // Determine virtual packages of the current platform
            let virtual_packages = VirtualPackage::current()
                .into_diagnostic()
                .context("failed to determine virtual packages")?
                .iter()
                .cloned()
                .map(GenericVirtualPackage::from)
                .collect();

            // Solve the environment
            let solver_matchspec = package_matchspec.clone();
            let solved_records = wrap_in_progress("solving environment", move || {
                Solver.solve(SolverTask {
                    specs: vec![solver_matchspec],
                    virtual_packages,
                    ..SolverTask::from_iter(specific_repodata)
                })
            })
            .into_diagnostic()
            .context("failed to solve environment")?;

            Ok((package_name, package_matchspec.clone(), solved_records))
        });
    }

    // Upgrade each package when relevant
    let mut upgraded = false;
    while let Some(data) = set.join_next().await {
        let (package_name, package_matchspec, records) = data.into_diagnostic()??;
        let toinstall_version = records
            .iter()
            .find(|r| r.package_record.name == package_name)
            .map(|p| p.package_record.version.version().to_owned())
            .ok_or_else(|| {
                miette::miette!(
                    "Package {} not found in the specified channels",
                    package_name.as_normalized()
                )
            })?;
        let installed_version = versions
            .get(&package_name)
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
            globally_install_package(
                &package_name,
                records,
                authenticated_client.clone(),
                platform,
            )
            .await?;
            pb.finish_with_message(format!("{} {}", console::style("Updated").green(), message));
            upgraded = true;
        }
    }

    if !upgraded {
        eprintln!("Nothing to upgrade");
    }

    Ok(())
}
