use std::{error::Error, path::PathBuf};

use clap::Parser;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_config::ConfigCli;
use pixi_utils::reqwest::build_reqwest_clients;
use rattler_shell::shell::ShellEnum;

use crate::global::{expose_add, expose_remove, BinDir, EnvRoot};

use crate::{
    global::{
        self, create_executable_scripts, script_exec_mapping, EnvDir, EnvironmentName, ExposedKey,
    },
    prefix::{create_activation_script, Prefix},
};

#[derive(Parser, Debug)]
pub struct AddArgs {
    /// The binary to add as executable in the form of key=value (e.g. python=python3.10)
    #[arg(value_parser = parse_key_val)]
    name: Vec<(String, String)>,

    #[clap(long)]
    environment: String,

    #[clap(flatten)]
    config: ConfigCli,
}

/// Parse a single key-value pair
fn parse_key_val(s: &str) -> Result<(String, String), Box<dyn Error + Send + Sync + 'static>> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=value: no `=` found in `{}`", s))?;
    let key = s[..pos].to_string();
    let value = s[pos + 1..].to_string();
    Ok((key, value))
}

#[derive(Parser, Debug)]
pub struct RemoveArgs {
    /// The binary or binaries to remove as executable  (e.g. python atuin)
    name: Vec<String>,

    #[clap(long)]
    environment: String,

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
    // should we do a sync first?
    let mut project = global::Project::discover()?.with_cli_config(args.config);
    let env_name: EnvironmentName = args.environment.parse()?;
    expose_add(&mut project, env_name, args.name).await?;

    let config = project.config();

    // after https://github.com/prefix-dev/pixi/pull/1975 PR lands
    // this can be simplified by just passing the project
    let (_, auth_client) = build_reqwest_clients(Some(&config));

    let gateway = config.gateway(auth_client.clone());

    let env_root = EnvRoot::from_env().await?;
    let bin_dir = BinDir::from_env().await?;


    global::sync(&env_root, &project, &bin_dir, config, &gateway, &auth_client).await
}

pub async fn remove(args: RemoveArgs) -> miette::Result<()> {
    // should we do a sync first?
    let mut project = global::Project::discover()?.with_cli_config(args.config);
    let env_name: EnvironmentName = args.environment.parse()?;
    expose_remove(&mut project, env_name, args.name).await?;

    let config = project.config();

    // after https://github.com/prefix-dev/pixi/pull/1975 PR lands
    // this can be simplified by just passing the project
    let (_, auth_client) = build_reqwest_clients(Some(&config));

    let gateway = config.gateway(auth_client.clone());

    let env_root = EnvRoot::from_env().await?;
    let bin_dir = BinDir::from_env().await?;


    global::sync(&env_root, &project, &bin_dir, config, &gateway, &auth_client).await

}
