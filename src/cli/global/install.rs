use clap::Parser;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{MatchSpec, NamedChannelOrUrl, Platform};

use crate::{
    cli::{global::revert_environment_after_error, has_specs::HasSpecs},
    global::{
        self, common::NotChangedReason, list::list_global_environments, project::ExposedType,
        EnvChanges, EnvState, EnvironmentName, Mapping, Project, StateChange, StateChanges,
    },
};
use pixi_config::{self, Config, ConfigCli};

/// Installs the defined packages in a globally accessible location and exposes their command line applications.
///
/// Example:
/// - pixi global install starship nushell ripgrep bat
/// - pixi global install jupyter --with polars
/// - pixi global install --expose python3.8=python python=3.8
/// - pixi global install --environment science --expose jupyter --expose ipython jupyter ipython polars
#[derive(Parser, Debug, Clone)]
#[clap(arg_required_else_help = true, verbatim_doc_comment)]
pub struct Args {
    /// Specifies the packages that are to be installed.
    #[arg(num_args = 1.., required = true)]
    packages: Vec<String>,

    /// The channels to consider as a name or a url.
    /// Multiple channels can be specified by using this field multiple times.
    ///
    /// When specifying a channel, it is common that the selected channel also
    /// depends on the `conda-forge` channel.
    ///
    /// By default, if no channel is provided, `conda-forge` is used.
    #[clap(long = "channel", short = 'c', value_name = "CHANNEL")]
    channels: Vec<NamedChannelOrUrl>,

    #[clap(short, long)]
    platform: Option<Platform>,

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
    config: ConfigCli,

    /// Specifies that the packages should be reinstalled even if they are already installed.
    #[arg(action, long)]
    force_reinstall: bool,
}

impl HasSpecs for Args {
    fn packages(&self) -> Vec<&str> {
        self.packages.iter().map(AsRef::as_ref).collect()
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project_original = global::Project::discover_or_create()
        .await?
        .with_cli_config(config.clone());

    let env_names = match &args.environment {
        Some(env_name) => Vec::from([env_name.clone()]),
        None => args
            .specs()?
            .iter()
            .map(|(package_name, _)| package_name.as_normalized().parse().into_diagnostic())
            .collect::<miette::Result<Vec<_>>>()?,
    };

    let multiple_envs = env_names.len() > 1;

    if !args.expose.is_empty() && env_names.len() != 1 {
        miette::bail!("Can't add exposed mappings with `--exposed` for more than one environment");
    }

    if !args.with.is_empty() && env_names.len() != 1 {
        miette::bail!("Can't add packages with `--with` for more than one environment");
    }

    let mut env_changes = EnvChanges::default();
    let mut last_updated_project = project_original;
    let specs = args.specs()?;
    for env_name in &env_names {
        let specs = if multiple_envs {
            specs
                .clone()
                .into_iter()
                .filter(|(package_name, _)| env_name.as_str() == package_name.as_source())
                .map(|(_, spec)| spec)
                .collect_vec()
        } else {
            specs
                .clone()
                .into_iter()
                .map(|(_, spec)| spec)
                .collect_vec()
        };
        let mut project = last_updated_project.clone();
        match setup_environment(env_name, &args, &specs, &mut project)
            .await
            .wrap_err_with(|| format!("Couldn't install {}", env_name.fancy_display()))
        {
            Ok(state_changes) => {
                if state_changes.has_changed() {
                    env_changes
                        .changes
                        .insert(env_name.clone(), EnvState::Installed)
                } else {
                    env_changes.changes.insert(
                        env_name.clone(),
                        EnvState::NotChanged(NotChangedReason::AlreadyInstalled),
                    )
                };
            }
            Err(err) => {
                revert_environment_after_error(env_name, &last_updated_project)
                    .await
                    .wrap_err("Couldn't install packages. Reverting also failed.")?;
                return Err(err);
            }
        }
        last_updated_project = project;
    }

    // After installing, we always want to list the changed environments
    list_global_environments(
        &last_updated_project,
        Some(env_names),
        Some(&env_changes),
        None,
    )
    .await?;

    Ok(())
}

async fn setup_environment(
    env_name: &EnvironmentName,
    args: &Args,
    specs: &[MatchSpec],
    project: &mut Project,
) -> miette::Result<StateChanges> {
    let mut state_changes = StateChanges::new_with_env(env_name.clone());

    let channels = if args.channels.is_empty() {
        project.config().default_channels()
    } else {
        args.channels.clone()
    };

    // Modify the project to include the new environment
    if !project.manifest.parsed.envs.contains_key(env_name) {
        project.manifest.add_environment(env_name, Some(channels))?;
        state_changes.insert_change(env_name, StateChange::AddedEnvironment);
    }

    if let Some(platform) = args.platform {
        project.manifest.set_platform(env_name, platform)?;
    }

    // Add the dependencies to the environment
    for spec in specs.iter().chain(&args.with) {
        project.manifest.add_dependency(
            env_name,
            spec,
            project.clone().config().global_channel_config(),
        )?;
    }

    if !args.expose.is_empty() {
        project.manifest.remove_all_exposed_mappings(env_name)?;
        // Only add the exposed mappings that were requested
        for mapping in &args.expose {
            project.manifest.add_exposed_mapping(env_name, mapping)?;
        }
    }

    if !args.force_reinstall && project.environment_in_sync(env_name).await? {
        return Ok(StateChanges::new_with_env(env_name.clone()));
    }

    // Installing the environment to be able to find the bin paths later
    let _ = project.install_environment(env_name).await?;

    let with_package_names = args
        .with
        .iter()
        .map(|spec| {
            spec.name
                .clone()
                .ok_or_else(|| miette::miette!("could not find package name in MatchSpec {}", spec))
        })
        .collect::<miette::Result<Vec<_>>>()?;

    // Sync exposed binaries
    let expose_type = if !args.expose.is_empty() {
        ExposedType::Mappings(args.expose.clone())
    } else if with_package_names.is_empty() {
        ExposedType::All
    } else {
        ExposedType::Ignore(with_package_names)
    };

    project.sync_exposed_names(env_name, expose_type).await?;

    // Figure out added packages and their corresponding versions
    state_changes |= project.added_packages(specs, env_name).await?;

    // Expose executables of the new environment
    state_changes |= project
        .expose_executables_from_environment(env_name)
        .await?;

    project.manifest.save().await?;
    Ok(state_changes)
}
