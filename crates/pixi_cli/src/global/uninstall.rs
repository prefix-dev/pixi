use crate::cli_interface::cli_global_context;
use crate::global::{EnvironmentAction, report_failed_environment, report_failed_environments};
use clap::Parser;
use miette::Report;
use pixi_api::global::{EnvironmentName, Project};
use pixi_config::{Config, ConfigCli};

/// Uninstalls environments from the global environment.
///
/// Example: `pixi global uninstall pixi-pack rattler-build`
#[derive(Parser, Debug, Clone)]
#[clap(arg_required_else_help = true, verbatim_doc_comment)]
pub struct Args {
    /// Specifies the environments that are to be removed.
    #[arg(num_args = 1.., required = true)]
    environment: Vec<EnvironmentName>,

    #[clap(flatten)]
    config: ConfigCli,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::try_with_cli_config(&args.config)?;
    let project = Project::discover_or_create()
        .await?
        .with_cli_config(config.clone());
    let mut context = cli_global_context(project);

    let mut errors: Vec<(EnvironmentName, Report)> = Vec::new();
    for env_name in &args.environment {
        match context.uninstall_environment(env_name).await {
            Ok(state_changes) => {
                state_changes.report(context.project()).await;
            }
            Err(err) => {
                report_failed_environment(env_name);
                errors.push((env_name.clone(), err.into()));
            }
        }
    }

    report_failed_environments(EnvironmentAction::Uninstall, errors)
}
