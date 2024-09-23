use std::str::FromStr;

use crate::global::{BinDir, EnvRoot};
use clap::Parser;
use pixi_config::{Config, ConfigCli};

use crate::global::{self, EnvironmentName, ExposedKey};

#[derive(Debug, Clone)]
struct Mapping {
    exposed_key: ExposedKey,
    executable_name: String,
}

impl Mapping {
    pub fn new(exposed_key: ExposedKey, executable_name: String) -> Self {
        Self {
            exposed_key,
            executable_name,
        }
    }
}

#[derive(Parser, Debug)]
pub struct AddArgs {
    /// The binary to add as executable in the form of key=value (e.g. python=python3.10)
    #[arg(value_parser = parse_mapping)]
    mapping: Vec<Mapping>,

    #[clap(short, long)]
    environment: EnvironmentName,

    /// Answer yes to all questions.
    #[clap(short = 'y', long = "yes", long = "assume-yes")]
    assume_yes: bool,

    #[clap(flatten)]
    config: ConfigCli,
}

/// Parse mapping between exposed key and executable name
fn parse_mapping(input: &str) -> miette::Result<Mapping> {
    input
        .split_once('=')
        .ok_or_else(|| {
            miette::miette!("Could not parse mapping `exposed_key=executable_name` from {input}")
        })
        .and_then(|(key, value)| Ok(Mapping::new(ExposedKey::from_str(key)?, value.to_string())))
}
#[derive(Parser, Debug)]
pub struct RemoveArgs {
    /// The binary or binaries to remove as executable  (e.g. python atuin)
    exposed_keys: Vec<ExposedKey>,

    #[clap(long)]
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

    let mut project = global::Project::discover_or_create(env_root, bin_dir, args.assume_yes)
        .await?
        .with_cli_config(config.clone());
    for mapping in args.mapping {
        project.manifest.add_exposed_binary(
            &args.environment,
            mapping.exposed_key,
            mapping.executable_name,
        )?;
    }

    project.manifest.save()?;

    global::sync(&project, &config).await
}

pub async fn remove(args: RemoveArgs) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);

    let bin_dir = BinDir::from_env().await?;
    let env_root = EnvRoot::from_env().await?;

    let mut project = global::Project::discover_or_create(env_root, bin_dir, args.assume_yes)
        .await?
        .with_cli_config(config.clone());
    for exposed_key in args.exposed_keys {
        project
            .manifest
            .remove_exposed_binary(&args.environment, &exposed_key)?;
    }

    project.manifest.save()?;

    global::sync(&project, &config).await
}
