use std::str::FromStr;

use indexmap::IndexMap;

use clap::Parser;
use miette::Report;
use rattler_conda_types::{MatchSpec, NamedChannelOrUrl, Subdir};

use crate::cli_interface::cli_global_context;
use crate::global::{
    EnvironmentAction, eventual_environment_channels, global_specs::GlobalSpecs,
    report_failed_environment, report_failed_environments,
};
use pixi_api::global::{EnvironmentName, GlobalSpec, InstallOptions, Mapping, Project};
use pixi_config::{self, Config, ConfigCli};

/// Installs the defined packages in a globally accessible location and exposes their command line applications.
///
/// Example:
///
/// - `pixi global install starship nushell ripgrep bat`
/// - `pixi global install jupyter --with polars`
/// - `pixi global install --expose python3.8=python python=3.8`
/// - `pixi global install --environment science --expose jupyter --expose ipython jupyter ipython polars`
#[derive(Parser, Debug, Clone, Default)]
#[clap(arg_required_else_help = true, verbatim_doc_comment)]
pub struct Args {
    /// Specifies the package that should be installed.
    #[clap(flatten)]
    pub packages: GlobalSpecs,

    /// The channels to consider as a name or a url.
    /// Multiple channels can be specified by using this field multiple times.
    ///
    /// When specifying a channel, it is common that the selected channel also
    /// depends on the `conda-forge` channel.
    ///
    /// By default, if no channel is provided, `conda-forge` is used.
    #[clap(long = "channel", short = 'c', value_name = "CHANNEL")]
    channels: Vec<NamedChannelOrUrl>,

    /// The platform to install the packages for.
    ///
    /// This is useful when you want to install packages for a different platform than the one you are currently on.
    /// This is very often used when you want to install `osx-64` packages on `osx-arm64`.
    #[clap(short, long)]
    platform: Option<Subdir>,

    /// Ensures that all packages will be installed in the same environment
    #[clap(short, long)]
    environment: Option<EnvironmentName>,

    /// Add one or more mapping which describe which executables are exposed.
    /// The syntax is `exposed_name=executable_name`, so for example `python3.10=python`.
    /// Alternatively, you can input only an executable_name and `executable_name=executable_name` is assumed.
    #[arg(long)]
    expose: Vec<Mapping>,

    /// Add additional dependencies to the environment.
    /// Their executables will not be exposed.
    #[arg(long)]
    with: Vec<MatchSpec>,

    #[clap(flatten)]
    pub config: ConfigCli,

    /// Specifies that the environment should be reinstalled.
    #[arg(action, long)]
    pub force_reinstall: bool,

    /// Specifies that no shortcuts should be created for the installed packages.
    #[arg(action, long, alias = "no-shortcut")]
    no_shortcuts: bool,

    /// Optional backend override (primarily for testing, not exposed in CLI)
    #[clap(skip)]
    pub backend_override: Option<pixi_build_frontend::BackendOverride>,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::try_with_cli_config(&args.config)?;

    // Load the global config and ensure
    // that the root_dir is relative to the manifest directory
    let mut project_original = Project::discover_or_create()
        .await?
        .with_cli_config(config.clone());

    // Apply backend override if provided (primarily for testing)
    if let Some(backend_override) = args.backend_override.clone() {
        project_original = project_original.with_backend_override(backend_override);
    }
    let channel_config = project_original.global_channel_config().clone();

    let inference_channels = eventual_environment_channels(
        &project_original,
        args.environment.as_ref(),
        &args.channels,
        args.force_reinstall,
    );
    let specs = args
        .packages
        .to_global_specs(
            &channel_config,
            &project_original.root,
            &project_original,
            &inference_channels,
        )
        .await?;
    let env_to_specs: IndexMap<EnvironmentName, Vec<GlobalSpec>> = match &args.environment {
        Some(env_name) => IndexMap::from([(env_name.clone(), specs)]),
        None => specs
            .into_iter()
            .map(|spec| {
                (
                    EnvironmentName::from_str(spec.name().as_normalized())
                        .expect("valid environment name"),
                    vec![spec],
                )
            })
            .collect(),
    };

    if !args.expose.is_empty() && env_to_specs.len() != 1 {
        miette::bail!("Can't add exposed mappings with `--exposed` for more than one environment");
    }

    if !args.with.is_empty() && env_to_specs.len() != 1 {
        miette::bail!("Can't add packages with `--with` for more than one environment");
    }

    let options = InstallOptions {
        channels: args.channels,
        platform: args.platform,
        expose: args.expose,
        with: args.with,
        force_reinstall: args.force_reinstall,
        no_shortcuts: args.no_shortcuts,
    };

    let mut context = cli_global_context(project_original);
    let mut errors: Vec<(EnvironmentName, Report)> = Vec::new();
    // Convert the packages into named global specs

    for (env_name, specs) in &env_to_specs {
        match context.install_environment(env_name, specs, &options).await {
            Ok(state_changes) => {
                // The user named these environments, so they get a block even
                // when nothing changed.
                state_changes.report(context.project()).await;
            }
            Err(err) => {
                report_failed_environment(env_name);
                errors.push((env_name.clone(), err.into()));
            }
        }
    }

    report_failed_environments(EnvironmentAction::Install, errors)
}
