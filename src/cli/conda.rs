use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use miette::{Context, IntoDiagnostic};
use pixi_config::{Config, ConfigCli};
use pixi_progress::{await_in_progress, global_multi_progress, wrap_in_progress};
use pixi_utils::{reqwest::build_reqwest_clients, PrefixGuard};
use rattler::{
    install::{IndicatifReporter, Installer},
    package_cache::PackageCache,
};
use rattler_conda_types::{GenericVirtualPackage, MatchSpec, Platform};
use rattler_solve::{resolvo::Solver, SolverImpl, SolverTask};
use rattler_virtual_packages::{VirtualPackage, VirtualPackageOverrides};
use reqwest_middleware::ClientWithMiddleware;

use crate::{prefix::Prefix};

use super::cli_config::ChannelsConfig;

#[derive(Parser, Debug)]
#[command(about = "A conda-compatible interface", long_about = None)]
pub struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Parser, Debug)]
struct CreateArgs {
    /// Name of the environment
    #[clap(long, short)]
    name: Option<String>,
    /// Path to the environment
    #[clap(long, short)]
    prefix: Option<PathBuf>,
    /// Path to a conda environment file (e.g. environment.yml)
    #[clap(long, short)]
    file: Option<PathBuf>,
    /// List of packages to install
    specs: Vec<MatchSpec>,

    #[clap(flatten)]
    channel: ChannelsConfig,

    #[clap(flatten)]
    config: ConfigCli,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Create a new conda environment
    Create(CreateArgs),
    /// List all conda environments
    List,
    /// Activate a conda environment
    Activate {
        /// Name of the environment
        name: String,
    },
    /// Deactivate the current conda environment
    Deactivate,
}

/// Creates a prefix for the `pixi conda ...` command.
pub async fn create_environment(
    prefix: &Path,
    channels: &ChannelsConfig,
    specs: Vec<MatchSpec>,
    cache_dir: &Path,
    config: &Config,
    client: &ClientWithMiddleware,
) -> miette::Result<Prefix> {
    let prefix = Prefix::new(prefix);

    let mut guard = PrefixGuard::new(prefix.root())
        .into_diagnostic()
        .context("failed to create prefix guard")?;

    let mut write_guard = wrap_in_progress("acquiring write lock on prefix", || guard.write())
        .into_diagnostic()
        .context("failed to acquire write lock to prefix guard")?;

    // If the environment already exists, and we are not forcing a
    // reinstallation, we can return early.
    if write_guard.is_ready() {
        // TODO: ask about overwriting
    }

    // Update the prefix to indicate that we are installing it.
    write_guard
        .begin()
        .into_diagnostic()
        .context("failed to write lock status to prefix guard")?;

    // Construct a gateway to get repodata.
    let gateway = config.gateway(client.clone());

    let channels = channels.resolve_from_config(config)?;

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

    // Determine virtual packages of the current platform
    let virtual_packages = VirtualPackage::detect(&VirtualPackageOverrides::from_env())
        .into_diagnostic()
        .context("failed to determine virtual packages")?
        .iter()
        .cloned()
        .map(GenericVirtualPackage::from)
        .collect();

    // Solve the environment
    tracing::info!(
        "creating environment in {}",
        dunce::canonicalize(prefix.root())
            .as_deref()
            .unwrap_or(prefix.root())
            .display()
    );
    let solved_records = wrap_in_progress("solving environment", move || {
        Solver.solve(SolverTask {
            specs,
            virtual_packages,
            ..SolverTask::from_iter(&repodata)
        })
    })
    .into_diagnostic()
    .context("failed to solve environment")?;

    // Install the environment
    Installer::new()
        .with_download_client(client.clone())
        .with_reporter(
            IndicatifReporter::builder()
                .with_multi_progress(global_multi_progress())
                .clear_when_done(true)
                .finish(),
        )
        .with_package_cache(PackageCache::new(
            cache_dir.join(pixi_consts::consts::CONDA_PACKAGE_CACHE_DIR),
        ))
        .install(prefix.root(), solved_records)
        .await
        .into_diagnostic()
        .context("failed to create environment")?;

    let _ = write_guard.finish();
    Ok(prefix)
}

async fn create(args: CreateArgs) -> miette::Result<()> {
    println!("Creating environment: {:?}", args);
    let cache_dir = pixi_config::get_cache_dir().context("failed to determine cache directory")?;
    let config = Config::with_cli_config(&args.config);
    let (_, client) = build_reqwest_clients(Some(&config));
    create_environment(
        &args.prefix.unwrap(),
        &args.channel,
        args.specs,
        &cache_dir,
        &config,
        &client,
    )
    .await?;

    Ok(())
}

pub async fn execute(args: Args) -> miette::Result<()> {
    match args.command {
        Commands::Create(args) => {
            create(args).await?;
        }
        Commands::List => {
            println!("Listing packages in active env");
        }
        Commands::Activate { name } => {
            println!("Activating environment: {:?}", name);
        }
        Commands::Deactivate => {
            println!("Deactivating environment");
        }
    }
    Ok(())
}
