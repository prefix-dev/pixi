use std::str::FromStr;

use clap::Parser;
use miette::Context;
use pixi_config::{Config, ConfigCli};

use crate::global::{self, EnvironmentName, ExposedName};

#[derive(Parser, Debug)]
pub struct AddArgs {
    /// Add one or more `MAPPING` for environment `ENV` which describe which executables are exposed.
    /// The syntax for `MAPPING` is `exposed_name=executable_name`, so for example `python3.10=python`.
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
    /// The exposed names that should be removed
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

async fn revert_after_error(
    mut project_original: global::Project,
    config: &Config,
) -> miette::Result<()> {
    project_original.manifest.save().await?;
    global::sync(&project_original, config).await?;
    Ok(())
}

pub async fn add(args: AddArgs) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project_original = global::Project::discover_or_create(args.assume_yes)
        .await?
        .with_cli_config(config.clone());

    async fn apply_changes(
        args: AddArgs,
        project_original: global::Project,
        config: &Config,
    ) -> Result<(), miette::Error> {
        let mut project_modified = project_original;

        for mapping in args.mappings {
            project_modified
                .manifest
                .add_exposed_mapping(&args.environment, &mapping)?;
        }
        project_modified.manifest.save().await?;
        global::sync(&project_modified, config).await?;
        Ok(())
    }

    if let Err(err) = apply_changes(args, project_original.clone(), &config).await {
        revert_after_error(project_original, &config)
            .await
            .wrap_err_with(|| format!("Reverting of the following error failed:\n {err}"))?;
        return Err(err);
    }
    Ok(())
}

pub async fn remove(args: RemoveArgs) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project_original = global::Project::discover_or_create(args.assume_yes)
        .await?
        .with_cli_config(config.clone());

    async fn apply_changes(
        args: RemoveArgs,
        project_original: global::Project,
        config: &Config,
    ) -> Result<(), miette::Error> {
        let mut project_modified = project_original;

        for exposed_name in args.exposed_names {
            project_modified
                .manifest
                .remove_exposed_name(&args.environment, &exposed_name)?;
        }
        project_modified.manifest.save().await?;
        global::sync(&project_modified, config).await?;
        Ok(())
    }

    if let Err(err) = apply_changes(args, project_original.clone(), &config).await {
        revert_after_error(project_original, &config)
            .await
            .wrap_err_with(|| format!("Reverting of the following error failed:\n {err}"))?;
        return Err(err);
    }
    Ok(())
}
