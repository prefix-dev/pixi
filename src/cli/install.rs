use crate::cli::cli_config::ProjectConfig;
use crate::environment::update_prefix;
use crate::Project;
use clap::Parser;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use pixi_config::ConfigCli;

/// Install all dependencies
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub project_config: ProjectConfig,

    #[clap(flatten)]
    pub lock_file_usage: super::LockFileUsageArgs,

    /// The environment to install
    #[arg(long, short)]
    pub environment: Option<Vec<String>>,

    #[clap(flatten)]
    pub config: ConfigCli,

    #[arg(long, short, conflicts_with = "environment")]
    pub all: bool,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.project_config.manifest_path.as_deref())?
        .with_cli_config(args.config);

    // Install either:
    //
    // 1. specific environments
    // 2. all environments
    // 3. default environment (if no environments are specified)
    let envs = if let Some(envs) = args.environment {
        envs
    } else if args.all {
        project
            .environments()
            .iter()
            .map(|env| env.name().to_string())
            .collect()
    } else {
        vec![project.default_environment().name().to_string()]
    };

    let mut installed_envs = Vec::with_capacity(envs.len());
    for env in envs {
        let environment = project.environment_from_name_or_env_var(Some(env))?;
        update_prefix(&environment, args.lock_file_usage.into(), false).await?;
        installed_envs.push(environment.name().clone());
    }

    // Message what's installed
    let detached_envs_message =
        if let Ok(Some(path)) = project.config().detached_environments().path() {
            format!(" in '{}'", console::style(path.display()).bold())
        } else {
            "".to_string()
        };

    if installed_envs.len() == 1 {
        eprintln!(
            "{}The {} environment has been installed{}.",
            console::style(console::Emoji("✔ ", "")).green(),
            installed_envs[0].fancy_display(),
            detached_envs_message
        );
    } else {
        eprintln!(
            "{}The following environments have been installed: {}\t{}",
            console::style(console::Emoji("✔ ", "")).green(),
            installed_envs.iter().map(|n| n.fancy_display()).join(", "),
            detached_envs_message
        );
    }

    Project::warn_on_discovered_from_env(args.project_config.manifest_path.as_deref());
    Ok(())
}
