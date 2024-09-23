use std::str::FromStr;

use crate::global::{BinDir, EnvRoot};
use clap::Parser;
use miette::Context;
use pixi_config::{Config, ConfigCli};

use crate::global::{self, EnvironmentName, ExposedName};

#[derive(Parser, Debug)]
pub struct AddArgs {
    /// The binary to add as executable in the form of key=value (e.g. python=python3.10)
    #[arg(value_parser = parse_mapping)]
    mappings: Vec<global::Mapping>,

    #[clap(short, long)]
    environment: EnvironmentName,

    /// Answer yes to all questions.
    #[clap(short = 'y', long = "yes", long = "assume-yes")]
    assume_yes: bool,

    #[clap(flatten)]
    config: ConfigCli,
}

/// Parse mapping between exposed name and executable name
fn parse_mapping(input: &str) -> miette::Result<global::Mapping> {
    input
        .split_once('=')
        .ok_or_else(|| {
            miette::miette!("Could not parse mapping `exposed_name=executable_name` from {input}")
        })
        .and_then(|(key, value)| {
            Ok(global::Mapping::new(
                ExposedName::from_str(key)?,
                value.to_string(),
            ))
        })
}
#[derive(Parser, Debug)]
pub struct RemoveArgs {
    /// The binary or binaries to remove as executable  (e.g. python atuin)
    exposed_names: Vec<ExposedName>,

    #[clap(short, long)]
    environment: EnvironmentName,

    /// Answer yes to all questions.
    #[clap(short = 'y', long = "yes", long = "assume-yes")]
    assume_yes: bool,

    #[clap(flatten)]
    config: ConfigCli,
}

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

    let bin_dir = BinDir::from_env().await?;
    let env_root = EnvRoot::from_env().await?;

    let mut project_original =
        global::Project::discover_or_create(env_root, bin_dir, args.assume_yes)
            .await?
            .with_cli_config(config.clone());

    // Make sure that the manifest is up-to-date with the local installation
    global::sync(&project_original, &config).await?;

    if let Err(err) = apply_add_changes(args, project_original.clone(), &config).await {
        let err = err.wrap_err("Could not add mappings");
        project_original
            .manifest
            .save()
            .await
            .wrap_err_with(|| format!("{}\nReverting also failed", &err))?;
        global::sync(&project_original, &config)
            .await
            .wrap_err_with(|| format!("{}\nReverting also failed", &err))?;
        return Err(err);
    }
    Ok(())
}

async fn apply_add_changes(
    args: AddArgs,
    project_original: global::Project,
    config: &Config,
) -> Result<(), miette::Error> {
    let mut project_modified = project_original.clone();

    for mapping in args.mappings {
        project_modified
            .manifest
            .add_exposed_mapping(&args.environment, &mapping)?;
    }
    project_modified.manifest.save().await?;
    global::sync(&project_modified, config).await?;
    Ok(())
}

pub async fn remove(args: RemoveArgs) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);

    let bin_dir = BinDir::from_env().await?;
    let env_root = EnvRoot::from_env().await?;

    let mut project_original =
        global::Project::discover_or_create(env_root, bin_dir, args.assume_yes)
            .await?
            .with_cli_config(config.clone());

    // Make sure that the manifest is up-to-date with the local installation
    global::sync(&project_original, &config).await?;

    if let Err(err) = apply_remove_changes(args, project_original.clone(), &config).await {
        let err = err.wrap_err("Could not remove exposed names");
        project_original
            .manifest
            .save()
            .await
            .wrap_err_with(|| format!("{}\nReverting also failed", &err))?;
        global::sync(&project_original, &config)
            .await
            .wrap_err_with(|| format!("{}\nReverting also failed", &err))?;
        return Err(err);
    }
    Ok(())
}

async fn apply_remove_changes(
    args: RemoveArgs,
    project_original: global::Project,
    config: &Config,
) -> Result<(), miette::Error> {
    let mut project_modified = project_original.clone();

    for exposed_name in args.exposed_names {
        project_modified
            .manifest
            .remove_exposed_name(&args.environment, &exposed_name)?;
    }
    project_modified.manifest.save().await?;
    global::sync(&project_modified, config).await?;
    Ok(())
}
