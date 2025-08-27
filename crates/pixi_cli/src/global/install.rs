use std::{ops::Not, str::FromStr};

use indexmap::IndexMap;

use clap::Parser;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{MatchSpec, NamedChannelOrUrl, Platform};

use crate::global::{global_specs::GlobalSpecs, revert_environment_after_error};
use pixi_config::{self, Config, ConfigCli};
use pixi_global::{
    self, EnvChanges, EnvState, EnvironmentName, Mapping, Project, StateChange, StateChanges,
    common::{NotChangedReason, contains_menuinst_document},
    list::list_all_global_environments,
    project::{ExposedType, GlobalSpec},
};

/// Installs the defined packages in a globally accessible location and exposes their command line applications.
///
/// Example:
///
/// - `pixi global install starship nushell ripgrep bat`
/// - `pixi global install jupyter --with polars`
/// - `pixi global install --expose python3.8=python python=3.8`
/// - `pixi global install --environment science --expose jupyter --expose ipython jupyter ipython polars`
#[derive(Parser, Debug, Clone)]
#[clap(arg_required_else_help = true, verbatim_doc_comment)]
pub struct Args {
    /// Specifies the package that should be installed.
    #[clap(flatten)]
    packages: GlobalSpecs,

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

    /// Specifies that the environment should be reinstalled.
    #[arg(action, long)]
    force_reinstall: bool,

    /// Specifies that no shortcuts should be created for the installed packages.
    #[arg(action, long, alias = "no-shortcut")]
    no_shortcuts: bool,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project_original = pixi_global::Project::discover_or_create()
        .await?
        .with_cli_config(config.clone());

    // Capture the current working directory for proper relative path resolution
    let current_dir = std::env::current_dir()
        .into_diagnostic()
        .wrap_err("Could not retrieve the current directory")?;
    let channel_config = rattler_conda_types::ChannelConfig {
        root_dir: current_dir,
        ..project_original.global_channel_config().clone()
    };

    let specs = args
        .packages
        .to_global_specs(&channel_config, &project_original.root, &project_original)
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

    let mut env_changes = EnvChanges::default();
    let mut last_updated_project = project_original;
    // Convert the packages into named global specs

    for (env_name, specs) in &env_to_specs {
        let mut project = last_updated_project.clone();
        match setup_environment(env_name, &args, specs, &mut project)
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
                if let Err(revert_err) =
                    revert_environment_after_error(env_name, &last_updated_project).await
                {
                    tracing::warn!("Reverting of the operation failed");
                    tracing::info!("Reversion error: {:?}", revert_err);
                }
                return Err(err);
            }
        }
        last_updated_project = project;
    }

    // After installing, we always want to list the changed environments
    list_all_global_environments(
        &last_updated_project,
        Some(env_to_specs.into_keys().collect()),
        Some(&env_changes),
        None,
        false,
    )
    .await?;

    Ok(())
}

async fn setup_environment(
    env_name: &EnvironmentName,
    args: &Args,
    specs: &[GlobalSpec],
    project: &mut Project,
) -> miette::Result<StateChanges> {
    let mut state_changes = StateChanges::new_with_env(env_name.clone());

    if args.force_reinstall && project.environment(env_name).is_some() {
        state_changes |= project.remove_environment(env_name).await?;
    }

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

    let converted_with_inclusions = args
        .with
        .iter()
        .map(|spec| {
            GlobalSpec::try_from_matchspec_with_name(
                spec.clone(),
                project.config().global_channel_config(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;

    // Add the dependencies to the environment
    let packages_to_add = specs
        .iter()
        .chain(converted_with_inclusions.iter())
        .collect_vec();

    for spec in &packages_to_add {
        project.manifest.add_dependency(env_name, spec)?;
    }

    if !args.expose.is_empty() {
        project.manifest.remove_all_exposed_mappings(env_name)?;
        // Only add the exposed mappings that were requested
        for mapping in &args.expose {
            project.manifest.add_exposed_mapping(env_name, mapping)?;
        }
    }

    if project.environment_in_sync_internal(env_name, true).await? {
        return Ok(StateChanges::new_with_env(env_name.clone()));
    }

    // Installing the environment to be able to find the bin paths later
    let environment_update = project.install_environment(env_name).await?;

    // Sync exposed name
    sync_exposed_names(env_name, project, args).await?;

    // Add shortcuts
    if !args.no_shortcuts {
        let prefix = project.environment_prefix(env_name).await?;
        for spec in specs.iter() {
            let prefix_record = prefix.find_designated_package(spec.name()).await?;
            if contains_menuinst_document(&prefix_record, prefix.root()) {
                project.manifest.add_shortcut(env_name, spec.name())?;
            }
        }
        state_changes |= project.sync_shortcuts(env_name).await?;
    }

    // Figure out added packages and their corresponding versions
    let requested_package_names: Vec<_> = packages_to_add
        .iter()
        .map(|spec| spec.name().clone())
        .collect();
    let user_requested_changes =
        environment_update.user_requested_changes(&requested_package_names);

    // Convert to StateChange::AddedPackage for packages that were installed or upgraded
    state_changes
        .add_packages_from_install_changes(env_name, user_requested_changes, project)
        .await?;

    // Expose executables of the new environment
    state_changes |= project
        .expose_executables_from_environment(env_name)
        .await?;

    // Sync completions
    state_changes |= project.sync_completions(env_name).await?;

    project.manifest.save().await?;
    Ok(state_changes)
}

async fn sync_exposed_names(
    env_name: &EnvironmentName,
    project: &mut Project,
    args: &Args,
) -> Result<(), miette::Error> {
    let with_package_names = args
        .with
        .iter()
        .map(|spec| {
            spec.name
                .clone()
                .ok_or_else(|| miette::miette!("could not find package name in MatchSpec {}", spec))
        })
        .collect::<miette::Result<Vec<_>>>()?;
    let expose_type = if args.expose.is_empty().not() {
        ExposedType::Mappings(args.expose.clone())
    } else if with_package_names.is_empty() {
        ExposedType::All
    } else {
        ExposedType::Ignore(with_package_names)
    };
    project.sync_exposed_names(env_name, expose_type).await?;
    Ok(())
}
