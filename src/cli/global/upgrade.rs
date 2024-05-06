use std::collections::HashMap;
use std::time::Duration;

use clap::Parser;
use indexmap::IndexMap;
use indicatif::ProgressBar;
use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::{Channel, MatchSpec, PackageName};

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
}

impl HasSpecs for Args {
    fn packages(&self) -> Vec<&str> {
        self.specs.iter().map(AsRef::as_ref).collect()
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::load_global();
    let specs = args.specs()?;
    upgrade_packages(specs, config, &args.channel).await
}

pub(super) async fn upgrade_packages(
    specs: IndexMap<PackageName, MatchSpec>,
    config: Config,
    cli_channels: &[String],
) -> miette::Result<()> {
    // Get channels and versions of globally installed packages
    let mut installed_versions = HashMap::with_capacity(specs.len());
    let mut channels = config.compute_channels(cli_channels).into_diagnostic()?;

    for package_name in specs.keys() {
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
    for (package_name, package_matchspec) in specs {
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
            globally_install_package(&package_name, records, authenticated_client.clone()).await?;
            pb.finish_with_message(format!("{} {}", console::style("Updated").green(), message));
            upgraded = true;
        }
    }

    if !upgraded {
        eprintln!("Nothing to upgrade");
    }

    Ok(())
}
