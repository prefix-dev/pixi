use crate::config::ConfigCli;
use crate::environment::get_up_to_date_prefix;
use crate::Project;
use clap::Parser;
use std::path::PathBuf;

/// Install all dependencies
#[derive(Parser, Debug)]
pub struct Args {
    /// The path to 'pixi.toml' or 'pyproject.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    #[clap(flatten)]
    pub lock_file_usage: super::LockFileUsageArgs,

    #[arg(long, short)]
    pub environment: Option<String>,

    #[clap(flatten)]
    pub config: ConfigCli,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project =
        Project::load_or_else_discover(args.manifest_path.as_deref())?.with_cli_config(args.config);
    let environment = project.environment_from_name_or_env_var(args.environment)?;

    get_up_to_date_prefix(&environment, args.lock_file_usage.into(), false).await?;

    // Emit success
    eprintln!(
        "{}Project in {} is ready to use!",
        console::style(console::Emoji("✔ ", "")).green(),
        project.root().display()
    );
    Project::warn_on_discovered_from_env(args.manifest_path.as_deref());
    Ok(())
}
