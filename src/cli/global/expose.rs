use std::{error::Error, path::PathBuf};

use clap::Parser;
use itertools::Itertools;
use pixi_config::ConfigCli;
use rattler_shell::shell::ShellEnum;

use crate::{
    global::{
        self, create_executable_scripts, script_exec_mapping, EnvDir, ExposedKey,
    },
    prefix::{create_activation_script, Prefix},
};

#[derive(Parser, Debug)]
pub struct AddArgs {
    /// The binary to add as executable in the form of key=value (e.g. python=python3.10)
    #[arg(value_parser = parse_key_val)]
    name: Vec<(String, String)>,

    #[clap(long)]
    environment_name: String,

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
#[clap(group(clap::ArgGroup::new("command")))]
pub enum Command {
    #[clap(name = "add")]
    Add(AddArgs),
}

/// Expose some binaries
pub async fn execute(args: Command) -> miette::Result<()> {
    match args {
        Command::Add(args) => add(args).await?,
    }
    Ok(())
}

pub async fn add(args: AddArgs) -> miette::Result<()> {
    // should we do a sync first?
    let mut project = global::Project::discover()?;

    let exposed_by_env = project.environment(args.environment_name.clone());

    if exposed_by_env.is_none() {
        miette::bail!("Environment not found");
    } else {
        exposed_by_env.expect("we checked this above");
    }

    let bin_env_dir = EnvDir::new(args.environment_name.clone()).await?;

    let prefix = Prefix::new(bin_env_dir.path());

    let prefix_records = prefix.find_installed_packages(None).await?;

    let all_executables: Vec<(String, PathBuf)> =
        prefix.find_executables(prefix_records.as_slice());

    let binary_to_be_exposed: Vec<&String> = args
        .name
        .iter()
        .map(|(_, actual_binary)| actual_binary)
        .collect();

    // Check if all binaries that are to be exposed are present in the environment
    let all_binaries_present = args
        .name
        .iter()
        .all(|(_, binary_name)| binary_to_be_exposed.contains(&binary_name));

    if !all_binaries_present {
        miette::bail!("Not all binaries are present in the environment");
    }

    let env_name = args.environment_name.clone().into();

    for (name_to_exposed, real_binary_to_be_exposed) in args.name.iter() {
        let exposed_key = ExposedKey::try_from(name_to_exposed.clone()).unwrap();

        let script_mapping = script_exec_mapping(
            &exposed_key,
            real_binary_to_be_exposed,
            all_executables.iter(),
            &bin_env_dir.bin_dir,
            &env_name,
        )?;

        // Determine the shell to use for the invocation script
        let shell: ShellEnum = if cfg!(windows) {
            rattler_shell::shell::CmdExe.into()
        } else {
            rattler_shell::shell::Bash.into()
        };

        let activation_script = create_activation_script(&prefix, shell.clone())?;

        create_executable_scripts(&[script_mapping], &prefix, &shell, activation_script).await?;

        // Add the new binary to the manifest
        project
            .manifest
            .expose_binary(
                &env_name,
                exposed_key,
                real_binary_to_be_exposed.to_string(),
            )
            .unwrap();
        project.manifest.save()?;
    }
    Ok(())
}
