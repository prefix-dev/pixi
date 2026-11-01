use std::process::ExitCode;

use clap::Parser;
use pixi_api::workspace::ShellOptions;
use pixi_config::{ConfigCli, ConfigCliActivation, ConfigCliPrompt};
use pixi_core::WorkspaceLocator;

use crate::cli_config::{LockAndInstallConfig, WorkspaceConfig};
use crate::cli_interface::cli_context;

/// Start a shell in a pixi environment.
///
/// Run `exit` to leave the shell.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,

    #[clap(flatten)]
    workspace_config: WorkspaceConfig,

    #[clap(flatten)]
    pub lock_and_install_config: LockAndInstallConfig,

    #[clap(flatten)]
    config: ConfigCli,

    /// The environment to activate in the shell
    #[arg(long, short)]
    environment: Option<String>,

    #[clap(flatten)]
    prompt_config: ConfigCliPrompt,

    #[clap(flatten)]
    activation_config: ConfigCliActivation,
}

pub async fn execute(args: Args) -> miette::Result<ExitCode> {
    let config = args
        .activation_config
        .merge_config(args.prompt_config.into())
        .merge_config(args.config.clone().into());

    let workspace = WorkspaceLocator::for_cli()
        .with_global_config_source(args.config_source.source())
        .with_search_start(args.workspace_config.workspace_locator_start())
        .with_cli_config(config)
        .locate()
        .await?;

    let environment = workspace
        .environment_from_name_or_env_var(args.environment)?
        .name()
        .clone();

    let options = ShellOptions {
        environment: Some(environment),
        lock_file_usage: args.lock_and_install_config.lock_file_usage()?,
        no_install: args.lock_and_install_config.no_install(),
    };

    let code = cli_context(workspace).shell(options).await?;
    Ok(crate::process_exit::exit_code_from_code(code))
}
