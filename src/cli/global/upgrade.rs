use itertools::Itertools;
use std::iter::once;
use std::{sync::Arc, time::Duration};

use clap::Parser;
use indexmap::IndexMap;
use indicatif::ProgressBar;
use miette::{IntoDiagnostic, Report};
use pixi_utils::reqwest::build_reqwest_clients;
use rattler_conda_types::{Channel, MatchSpec, PackageName, Platform};

use tokio::task::JoinSet;

use super::{common::find_installed_package, install::globally_install_package};
use crate::cli::{
    cli_config::ChannelsConfig, global::common::solve_package_records, has_specs::HasSpecs,
};
use pixi_config::Config;
use pixi_progress::{global_multi_progress, long_running_progress_style};

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
    let (_, client) = build_reqwest_clients(Some(&config));
    let gateway = config.gateway(client.clone());

    // Resolve environments in parallel
    let mut set: JoinSet<Result<_, Report>> = JoinSet::new();
    // Create arcs for these structs
    // as they later will be captured by closure
    let channel_config = Arc::new(config.global_channel_config().clone());
    let channel_cli = Arc::new(channel_cli);

    for (package_name, package_matchspec) in specs {
        let channel_config = channel_config.clone();
        let channel_cli = channel_cli.clone();
        let gateway = gateway.clone(); // Already an Arc under the hood

        set.spawn(async move {
            let record = find_installed_package(&package_name).await?.repodata_record;
            let channel = Channel::from_str(record.channel, &channel_config).into_diagnostic()?;
            let version = record.package_record.version.into_version();

            let channels = channel_cli
                .iter()
                .cloned()
                .chain(once(channel).into_iter())
                .unique();
            let records = solve_package_records(
                &gateway,
                platform,
                channels,
                vec![package_matchspec.clone()],
            )
            .await?;

            Ok((package_name, package_matchspec, records, version))
        });
    }

    // Upgrade each package when relevant
    let mut upgraded = false;
    while let Some(data) = set.join_next().await {
        let (package_name, package_matchspec, records, installed_version) =
            data.into_diagnostic()??;
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
            globally_install_package(&package_name, records, client.clone(), platform).await?;
            pb.finish_with_message(format!("{} {}", console::style("Updated").green(), message));
            upgraded = true;
        }
    }

    if !upgraded {
        eprintln!("Nothing to upgrade");
    }

    Ok(())
}
