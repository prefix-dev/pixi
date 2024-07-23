use std::{
    hash::{DefaultHasher, Hash, Hasher},
    path::Path,
    str::FromStr,
};

use clap::{Parser, ValueHint};
use miette::{Context, IntoDiagnostic};
use rattler::{
    install::{IndicatifReporter, Installer},
    package_cache::PackageCache,
};
use rattler_conda_types::{
    ChannelConfig, GenericVirtualPackage, MatchSpec, NamedChannelOrUrl, PackageName,
    Platform,
};
use rattler_repodata_gateway::Gateway;
use rattler_solve::{resolvo::Solver, SolverImpl, SolverTask};
use rattler_virtual_packages::VirtualPackage;
use reqwest_middleware::ClientWithMiddleware;

use crate::{
    config::{self, Config, ConfigCli},
    prefix::Prefix,
    progress::{await_in_progress, global_multi_progress, wrap_in_progress},
    utils::{reqwest::build_reqwest_clients, PrefixGuard},
};

/// Run a command in a temporary environment.
#[derive(Parser, Debug, Default)]
#[clap(trailing_var_arg = true, arg_required_else_help = true)]
pub struct Args {
    /// The executable to run.
    #[clap(num_args = 1.., value_hint = ValueHint::CommandWithArguments)]
    pub command: Vec<String>,

    /// Matchspecs of packages to install. If this is not provided, the package
    /// is guessed from the command.
    #[clap(long = "spec", short = 's')]
    pub specs: Vec<MatchSpec>,

    /// The channel to install the packages from.
    #[clap(long = "channel", short = 'c')]
    pub channels: Vec<NamedChannelOrUrl>,

    /// If specified a new environment is always created even if one already
    /// exists.
    #[clap(long)]
    pub force_reinstall: bool,

    #[clap(flatten)]
    pub config: ConfigCli,
}

#[derive(Hash)]
pub struct EnvironmentHash {
    pub command: String,
    pub specs: Vec<MatchSpec>,
    pub channels: Vec<String>,
}

impl EnvironmentHash {
    pub fn from_args(args: &Args, channel_config: &ChannelConfig) -> Self {
        Self {
            command: args
                .command
                .first()
                .cloned()
                .expect("missing required command"),
            specs: args.specs.clone(),
            channels: args
                .channels
                .iter()
                .map(|c| {
                    c.clone()
                        .into_channel(channel_config)
                        .base_url()
                        .to_string()
                })
                .collect(),
        }
    }

    /// Returns the name of the environment.
    pub fn name(&self) -> String {
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        let hash = hasher.finish();
        format!("{}-{:x}", &self.command, hash)
    }
}

/// CLI entry point for `pixi runx`
pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let cache_dir = config::get_cache_dir().context("failed to determine cache directory")?;

    let mut command_args = args.command.iter();
    let command = command_args.next().ok_or_else(|| miette::miette!(help ="i.e when specifying specs explicitly use a command at the end: `pixi exec -s python==3.12 python`", "missing required command to execute",))?;
    let (_, client) = build_reqwest_clients(Some(&config));

    // Create the environment to run the command in.
    let prefix = create_exec_prefix(&args, &cache_dir, &config, &client).await?;

    // Get environment variables from the activation
    let activation_env = run_activation(&prefix).await?;

    // Ignore CTRL+C so that the child is responsible for its own signal handling.
    let _ctrl_c = tokio::spawn(async { while tokio::signal::ctrl_c().await.is_ok() {} });

    // Spawn the command
    let status = std::process::Command::new(command)
        .args(command_args)
        .envs(activation_env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .status()
        .into_diagnostic()
        .with_context(|| format!("failed to execute '{}'", &command))?;

    // Return the exit code of the command
    std::process::exit(status.code().unwrap_or(1));
}

/// Creates a prefix for the `pixi exec` command.
pub async fn create_exec_prefix(
    args: &Args,
    cache_dir: &Path,
    config: &Config,
    client: &ClientWithMiddleware,
) -> miette::Result<Prefix> {
    let environment_name = EnvironmentHash::from_args(args, config.channel_config()).name();
    let prefix = Prefix::new(cache_dir.join("cached-envs-v0").join(environment_name));

    let mut guard = PrefixGuard::new(prefix.root())
        .into_diagnostic()
        .context("failed to create prefix guard")?;

    let mut write_guard = wrap_in_progress("acquiring write lock on prefix", || guard.write())
        .into_diagnostic()
        .context("failed to acquire write lock to prefix guard")?;

    // If the environment already exists, and we are not forcing a
    // reinstallation, we can return early.
    if write_guard.is_ready() && !args.force_reinstall {
        tracing::info!(
            "reusing existing environment in {}",
            prefix.root().display()
        );
        let _ = write_guard.finish();
        return Ok(prefix);
    }

    // Update the prefix to indicate that we are installing it.
    write_guard
        .begin()
        .into_diagnostic()
        .context("failed to write lock status to prefix guard")?;

    // Construct a gateway to get repodata.
    let gateway = Gateway::builder()
        .with_cache_dir(cache_dir.join("repodata"))
        .with_client(client.clone())
        .with_channel_config(rattler_repodata_gateway::ChannelConfig::from(config))
        .finish();

    // Determine the specs to use for the environment
    let specs = if args.specs.is_empty() {
        let command = args.command.first().expect("missing required command");
        let guessed_spec = guess_package_spec(command);

        tracing::debug!(
            "no specs provided, guessed {} from command {command}",
            guessed_spec
        );

        vec![guessed_spec]
    } else {
        args.specs.clone()
    };

    // Parse the channels
    let channels = if args.channels.is_empty() {
        config.default_channels()
    } else {
        args.channels.clone()
    };
    let channels = channels
        .into_iter()
        .map(|channel| channel.into_channel(config.channel_config()));

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
    let virtual_packages = VirtualPackage::current()
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
        .with_reporter(
            IndicatifReporter::builder()
                .with_multi_progress(global_multi_progress())
                .clear_when_done(true)
                .finish(),
        )
        .with_package_cache(PackageCache::new(cache_dir.join("pkgs")))
        .install(prefix.root(), solved_records)
        .await
        .into_diagnostic()
        .context("failed to create environment")?;

    let _ = write_guard.finish();
    Ok(prefix)
}

/// This function is used to guess the package name from the command.
fn guess_package_spec(command: &str) -> MatchSpec {
    // Replace any illegal character with a dash.
    // TODO: In the future it would be cool if we look at all packages available
    //  and try to find the closest match.
    let command = command.replace(
        |c| !matches!(c, 'a'..='z'|'A'..='Z'|'0'..='9'|'-'|'_'|'.'),
        "-",
    );

    MatchSpec {
        name: Some(PackageName::from_str(&command).expect("all illegal characters were removed")),
        ..Default::default()
    }
}

/// Run the activation scripts of the prefix.
async fn run_activation(
    prefix: &Prefix,
) -> miette::Result<std::collections::HashMap<String, String>> {
    wrap_in_progress("running activation", move || prefix.run_activation()).await
}
