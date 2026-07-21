use std::path::PathBuf;

use clap::Parser;
use pixi_config::{ConfigCli, ConfigCliActivation};

use crate::cli_config::LockAndInstallConfig;

/// Run a script in an isolated Pixi environment.
#[derive(Debug, Parser)]
#[clap(trailing_var_arg = true, disable_help_flag = true)]
pub struct Args {
    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,

    #[clap(flatten)]
    pub lock_and_install_config: LockAndInstallConfig,

    #[clap(flatten)]
    pub config: ConfigCli,

    #[clap(flatten)]
    pub activation_config: ConfigCliActivation,

    /// Use a minimal environment while activating the script environment.
    #[arg(long)]
    pub clean_env: bool,

    /// Script to run followed by arguments forwarded to it.
    #[arg(required = true, num_args = 1.., allow_hyphen_values = true)]
    pub command: Vec<String>,

    #[clap(long, action = clap::ArgAction::HelpLong)]
    pub help: Option<bool>,

    #[clap(short, action = clap::ArgAction::HelpShort)]
    pub h: Option<bool>,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let mut command = args.command.into_iter();
    let path = PathBuf::from(
        command
            .next()
            .expect("clap requires at least the script path"),
    );
    crate::run::execute(crate::run::Args {
        config_source: args.config_source,
        task: command.collect(),
        script: Some(path),
        lock_and_install_config: args.lock_and_install_config,
        config: args.config,
        activation_config: args.activation_config,
        clean_env: args.clean_env,
        ..Default::default()
    })
    .await
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::Args;

    #[test]
    fn requires_an_explicit_run_and_forwards_trailing_arguments() {
        let args = Args::try_parse_from(["run", "example.py", "first", "--second"]).unwrap();
        assert_eq!(args.command, ["example.py", "first", "--second"]);
    }

    #[test]
    fn pixi_options_must_precede_the_script_path() {
        let args = Args::try_parse_from(["run", "--frozen", "example.py"]).unwrap();
        assert!(args.lock_and_install_config.lock_file_usage().is_ok());

        let args = Args::try_parse_from(["run", "example.py", "--frozen"]).unwrap();
        assert_eq!(args.command, ["example.py", "--frozen"]);
    }
}
