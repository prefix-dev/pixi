use clap::Parser;
use miette::Context;
use pixi_config::{Config, ConfigCli};

use crate::{
    cli::global::revert_environment_after_error,
    global::{self, EnvironmentName, ExposedName, Mapping, StateChanges},
};

/// Add exposed binaries from an environment to your global environment
///
/// `pixi global expose add python310=python3.10 python3=python3 --environment myenv`
/// will expose the `python3.10` executable as `python310` and the `python3` executable as `python3`
#[derive(Parser, Debug)]
pub struct AddArgs {
    /// Add one or more mapping which describe which executables are exposed.
    /// The syntax is `exposed_name=executable_name`, so for example `python3.10=python`.
    /// Alternatively, you can input only an executable_name and `executable_name=executable_name` is assumed.
    #[arg(num_args = 1..)]
    mappings: Vec<Mapping>,

    /// The environment to which the binaries should be exposed
    #[clap(short, long)]
    environment: EnvironmentName,

    /// Answer yes to all questions.
    #[clap(short = 'y', long = "yes", long = "assume-yes")]
    assume_yes: bool,

    #[clap(flatten)]
    config: ConfigCli,
}

/// Remove exposed binaries from the global environment
///
/// `pixi global expose remove python310 python3 --environment myenv`
/// will remove the exposed names `python310` and `python3` from the environment `myenv`
#[derive(Parser, Debug)]
pub struct RemoveArgs {
    /// The exposed names that should be removed
    #[arg(num_args = 1..)]
    exposed_names: Vec<ExposedName>,

    /// The environment from which the exposed names should be removed
    #[clap(short, long)]
    environment: EnvironmentName,

    /// Answer yes to all questions.
    #[clap(short = 'y', long = "yes", long = "assume-yes")]
    assume_yes: bool,

    #[clap(flatten)]
    config: ConfigCli,
}

/// Interact with the exposure of binaries in the global environment
///
/// `pixi global expose add python310=python3.10 --environment myenv`
/// will expose the `python3.10` executable as `python310` from the environment `myenv`
///
/// `pixi global expose remove python310 --environment myenv`
/// will remove the exposed name `python310` from the environment `myenv`
#[derive(Parser, Debug)]
#[clap(group(clap::ArgGroup::new("command")))]
pub enum SubCommand {
    #[clap(name = "add")]
    Add(AddArgs),
    #[clap(name = "remove")]
    Remove(RemoveArgs),
}

/// Expose some binaries
pub async fn execute(args: SubCommand) -> miette::Result<()> {
    match args {
        SubCommand::Add(args) => add(args).await?,
        SubCommand::Remove(args) => remove(args).await?,
    }
    Ok(())
}

pub async fn add(args: AddArgs) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project_original = global::Project::discover_or_create(args.assume_yes)
        .await?
        .with_cli_config(config.clone());

    async fn apply_changes(
        args: &AddArgs,
        project_modified: &mut global::Project,
        state_changes: &mut StateChanges,
    ) -> Result<(), miette::Error> {
        let env_name = &args.environment;
        for mapping in &args.mappings {
            project_modified
                .manifest
                .add_exposed_mapping(env_name, mapping)?;
        }
        *state_changes |= project_modified.sync_environment(env_name).await?;
        project_modified.manifest.save().await?;
        Ok(())
    }

    let mut project_modified = project_original.clone();
    let mut state_changes = StateChanges::default();
    if let Err(err) = apply_changes(&args, &mut project_modified, &mut state_changes).await {
        state_changes.report();
        revert_environment_after_error(&args.environment, &project_original)
            .await
            .wrap_err("Could not add exposed mappings. Reverting also failed.")?;
        Err(err)
    } else {
        state_changes.report();
        Ok(())
    }
}

pub async fn remove(args: RemoveArgs) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project_original = global::Project::discover_or_create(args.assume_yes)
        .await?
        .with_cli_config(config.clone());

    async fn apply_changes(
        args: &RemoveArgs,
        project_modified: &mut global::Project,
        state_changes: &mut StateChanges,
    ) -> Result<(), miette::Error> {
        let env_name = &args.environment;
        for exposed_name in &args.exposed_names {
            project_modified
                .manifest
                .remove_exposed_name(env_name, exposed_name)?;
        }
        *state_changes |= project_modified.sync_environment(env_name).await?;
        project_modified.manifest.save().await?;
        Ok(())
    }

    let mut project_modified = project_original.clone();
    let mut state_changes = StateChanges::default();

    if let Err(err) = apply_changes(&args, &mut project_modified, &mut state_changes).await {
        state_changes.report();
        revert_environment_after_error(&args.environment, &project_original)
            .await
            .wrap_err("Could not remove exposed name. Reverting also failed.")?;
        Err(err)
    } else {
        state_changes.report();
        Ok(())
    }
}
