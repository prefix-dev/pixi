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
use rattler_conda_types::{Channel, GenericVirtualPackage, MatchSpec, PackageName, Platform};
use rattler_repodata_gateway::{ChannelConfig, Gateway};
use rattler_solve::{resolvo::Solver, SolverImpl, SolverTask};
use rattler_virtual_packages::VirtualPackage;

use crate::{
    config,
    config::{Config, ConfigCli},
    prefix::Prefix,
    progress::{await_in_progress, global_multi_progress, wrap_in_progress},
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
    pub channels: Vec<String>,

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

impl<'a> From<&'a Args> for EnvironmentHash {
    fn from(value: &'a Args) -> Self {
        Self {
            command: value
                .command
                .first()
                .cloned()
                .expect("missing required command"),
            specs: value.specs.clone(),
            channels: value.channels.clone(),
        }
    }
}

impl EnvironmentHash {
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

    // Create the environment to run the command in.
    let prefix = create_prefix(&args, &cache_dir, &config).await?;

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

pub async fn create_prefix(
    args: &Args,
    cache_dir: &Path,
    config: &Config,
) -> miette::Result<Prefix> {
    let environment_name = EnvironmentHash::from(args).name();
    let prefix = Prefix::new(cache_dir.join("cached-envs-v0").join(environment_name));

    // If the environment already exists, and we are not forcing a
    // reinstallation, we can return early.
    if prefix.root().exists() && !args.force_reinstall {
        return Ok(prefix);
    }

    // Construct a gateway to get repodata.
    let gateway = Gateway::builder()
        .with_cache_dir(cache_dir.join("repodata"))
        .with_channel_config(ChannelConfig::from(config))
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
        .iter()
        .map(|channel| Channel::from_str(channel, config.channel_config()))
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()?;

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
