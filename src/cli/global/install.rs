use clap::Parser;
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use pixi_utils::reqwest::build_reqwest_clients;
use rattler_conda_types::{GenericVirtualPackage, Platform};

use rattler_solve::{resolvo::Solver, SolverImpl, SolverTask};
use rattler_virtual_packages::VirtualPackage;

use crate::global::{
    channel_name_from_prefix,
    install::{globally_install_package, prompt_user_to_continue},
    print_executables_available,
};
use crate::{cli::cli_config::ChannelsConfig, cli::has_specs::HasSpecs};
use pixi_config::{self, Config, ConfigCli};
use pixi_progress::wrap_in_progress;

/// Installs the defined package in a global accessible location.
#[derive(Parser, Debug)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    /// Specifies the packages that are to be installed.
    #[arg(num_args = 1..)]
    packages: Vec<String>,

    #[clap(flatten)]
    channels: ChannelsConfig,

    #[clap(short, long, default_value_t = Platform::current())]
    platform: Platform,

    #[clap(flatten)]
    config: ConfigCli,
}

impl HasSpecs for Args {
    fn packages(&self) -> Vec<&str> {
        self.packages.iter().map(AsRef::as_ref).collect()
    }
}

/// Install a global command
pub async fn execute(args: Args) -> miette::Result<()> {
    // Figure out what channels we are using
    let config = Config::with_cli_config(&args.config);
    let channels = args.channels.resolve_from_config(&config);

    let specs = args.specs()?;

    // Warn user on dangerous package installations, interactive yes no prompt
    if !prompt_user_to_continue(&specs)? {
        return Ok(());
    }

    // Fetch the repodata
    let (_, auth_client) = build_reqwest_clients(Some(&config));

    let gateway = config.gateway(auth_client.clone());

    let repodata = gateway
        .query(
            channels,
            [args.platform, Platform::NoArch],
            specs.values().cloned().collect_vec(),
        )
        .recursive(true)
        .await
        .into_diagnostic()?;

    // Determine virtual packages of the current platform
    let virtual_packages = VirtualPackage::current()
        .into_diagnostic()
        .context("failed to determine virtual packages")?
        .iter()
        .cloned()
        .map(GenericVirtualPackage::from)
        .collect();

    // Solve the environment
    let solver_specs = specs.clone();
    let solved_records = wrap_in_progress("solving environment", move || {
        Solver.solve(SolverTask {
            specs: solver_specs.values().cloned().collect_vec(),
            virtual_packages,
            ..SolverTask::from_iter(&repodata)
        })
    })
    .into_diagnostic()
    .context("failed to solve environment")?;

    // Install the package(s)
    let mut executables = vec![];
    for (package_name, _) in specs {
        let (prefix_package, scripts, _) = globally_install_package(
            &package_name,
            solved_records.clone(),
            auth_client.clone(),
            args.platform,
        )
        .await?;
        let channel_name =
            channel_name_from_prefix(&prefix_package, config.global_channel_config());
        let record = &prefix_package.repodata_record.package_record;

        // Warn if no executables were created for the package
        if scripts.is_empty() {
            eprintln!(
                "{}No executable entrypoint found in package {}, are you sure it exists?",
                console::style(console::Emoji("⚠️", "")).yellow().bold(),
                console::style(record.name.as_source()).bold()
            );
        }

        eprintln!(
            "{}Installed package {} {} {} from {}",
            console::style(console::Emoji("✔ ", "")).green(),
            console::style(record.name.as_source()).bold(),
            console::style(record.version.version()).bold(),
            console::style(record.build.as_str()).bold(),
            channel_name,
        );

        executables.extend(scripts);
    }

    if !executables.is_empty() {
        print_executables_available(executables).await?;
    }

    Ok(())
}
