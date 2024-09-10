use std::{collections::HashMap, path::PathBuf};

use clap::Parser;
use itertools::Itertools;
use rattler_shell::shell::ShellEnum;

use crate::{
    global::{
        self, create_executable_scripts, script_exec_mapping, BinDir, EnvDir, EnvRoot,
        EnvironmentName, ExposedKey,
    },
    prefix::{create_activation_script, Prefix},
};

#[derive(Parser, Debug)]
pub struct AddArgs {
    /// The binary to add as executable in the form of key=value (e.g. python=python3.10)
    #[arg(value_parser = parse_key_val)]
    name: HashMap<String, String>,

    #[clap(long)]
    environment_name: String,
}

/// Custom parser to split the input into a key-value pair
fn parse_key_val(s: &str) -> Result<(String, String), String> {
    let parts: Vec<&str> = s.splitn(2, '=').collect();
    if parts.len() != 2 {
        return Err(format!("Invalid format: {}", s));
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
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
        Command::Add(args) => {
            let mut project = global::Project::discover()?;

            let exposed_by_env = project.environment(args.environment_name.clone());

            if let None = exposed_by_env{
                miette::bail!("Environment not found");
            } else {
                exposed_by_env.expect("we checked this above");
            }


            let bin_env_dir = EnvDir::new(args.environment_name.clone()).await?;

            let prefix = Prefix::new(bin_env_dir.path());

            let prefix_records = prefix.find_installed_packages(None).await?;

            let all_executables: Vec<(String, PathBuf)> =
                prefix.find_executables(prefix_records.as_slice());


            // let exposed = exposed_by_env.exposed;

            // add the new executable
            let exposed_key = ExposedKey::try_from(args.name.clone()).unwrap();
            let env_name = EnvironmentName::from(args.environment_name.clone());

            let script_mapping = script_exec_mapping(
                &exposed_key,
                &args.name.clone(),
                all_executables,
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

            create_executable_scripts(&[script_mapping], &prefix, &shell, activation_script)
                .await?;

            // Add the new binary to the manifest
            // project.manifest.expose_binary(args.environment_name, args.name).unwrap();
        }
    }
    Ok(())

}
