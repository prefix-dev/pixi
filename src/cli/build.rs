use crate::config::ConfigCli;
use crate::prefix::Prefix;
use crate::progress::{await_in_progress, global_multi_progress, wrap_in_progress};
use crate::utils::reqwest::build_reqwest_clients;
use crate::{config, HasFeatures, Project};
use clap::Parser;
use itertools::Itertools;
use miette::{miette, Context, IntoDiagnostic};
use rattler::install::{IndicatifReporter, Installer};
use rattler::package_cache::PackageCache;
use rattler_conda_types::{GenericVirtualPackage, MatchSpec, Platform};
use rattler_repodata_gateway::{ChannelConfig, Gateway};
use rattler_solve::resolvo::Solver;
use rattler_solve::{SolverImpl, SolverTask};
use rattler_virtual_packages::VirtualPackage;
use std::path::PathBuf;

/// Invoke the build command
#[derive(Parser, Debug, Default)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    /// The build to run
    pub name: String,

    /// The path to 'pixi.toml' or 'pyproject.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    #[clap(flatten)]
    pub config: ConfigCli,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project =
        Project::load_or_else_discover(args.manifest_path.as_deref())?.with_cli_config(args.config);

    // Find the build to run
    let build = project
        .manifest
        .build(args.name.clone())
        .ok_or_else(|| miette::miette!("No build with name: '{}' found", args.name))?;

    let specs = build
        .dependencies
        .iter()
        .map(|(name, spec)| MatchSpec::from_nameless(spec.clone(), Some(name.clone())))
        .collect_vec();

    // Create environment for build
    let environment_name = format!("build-{}-v0", build.name.clone());
    let prefix = Prefix::new(project.environments_dir().join(environment_name));

    // Construct a gateway to get repodata.
    let (_, client) = build_reqwest_clients(Some(project.config()));
    let gateway = Gateway::builder()
        .with_cache_dir(config::get_cache_dir()?.join("repodata"))
        .with_client(client.clone())
        .with_channel_config(ChannelConfig::from(project.config()))
        .finish();

    // Determine virtual packages of the current platform
    let virtual_packages = VirtualPackage::current()
        .into_diagnostic()
        .context("failed to determine virtual packages")?
        .iter()
        .cloned()
        .map(GenericVirtualPackage::from)
        .collect();

    // Get channels
    let channels = project
        .default_environment()
        .channels()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();

    // Get the repodata for the specs
    let repodata = await_in_progress("fetching repodata for environment", |_| async {
        gateway
            .query(
                channels,
                [Platform::current(), Platform::NoArch],
                specs.clone(),
            )
            .recursive(true)
            .execute()
            .await
    })
    .await
    .into_diagnostic()
    .context("failed to get repodata")?;

    // Solve the environment
    tracing::info!(
        "creating build environment in {}",
        dunce::canonicalize(prefix.root())
            .as_deref()
            .unwrap_or(prefix.root())
            .display()
    );
    let solved_records = wrap_in_progress("solving build environment", move || {
        Solver.solve(SolverTask {
            specs,
            virtual_packages,
            ..SolverTask::from_iter(&repodata)
        })
    })
    .into_diagnostic()
    .context("failed to solve build environment")?;

    // Install the environment
    let package_cache = PackageCache::new(config::get_cache_dir()?.join("pkgs"));
    Installer::new()
        .with_reporter(
            IndicatifReporter::builder()
                .with_multi_progress(global_multi_progress())
                .clear_when_done(true)
                .finish(),
        )
        .with_package_cache(package_cache)
        .install(prefix.root(), solved_records)
        .await
        .into_diagnostic()
        .context("failed to create build environment")?;

    // Get environment variables from the activation
    let activation_env = run_activation(&prefix).await?;

    // Ignore CTRL+C so that the child is responsible for its own signal handling.
    let _ctrl_c = tokio::spawn(async { while tokio::signal::ctrl_c().await.is_ok() {} });

    // TODO: THIS IS NOT A TASK. for quick testing this is now a command.
    if let Some(command) = build.task.as_single_command() {
        let command = command
            .split_whitespace()
            .next()
            .ok_or_else(|| miette!("No command found"))?;

        let status = std::process::Command::new(command)
            .envs(activation_env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .status()
            .into_diagnostic()
            .with_context(|| format!("failed to execute '{}'", command))?;
        // Return the exit code of the command
        std::process::exit(status.code().unwrap_or(1));
    } else {
        Err(miette!("Only single command tasks are supported for now"))
    }
}

/// TODO: I don't understand the need for this function. But without it, the compiler screams async jibberish at me(ruben)
/// Run the activation scripts of the prefix.
async fn run_activation(
    prefix: &Prefix,
) -> miette::Result<std::collections::HashMap<String, String>> {
    wrap_in_progress("running activation", move || prefix.run_activation()).await
}
