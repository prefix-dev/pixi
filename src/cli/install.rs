use crate::environment::get_up_to_date_prefix;
use crate::project::manifest::EnvironmentName;
use crate::task::TaskName;
use crate::Project;
use clap::Parser;
use indexmap::IndexMap;
use std::path::PathBuf;

/// Install all dependencies
#[derive(Parser, Debug)]
pub struct Args {
    /// The path to 'pixi.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    #[clap(flatten)]
    pub lock_file_usage: super::LockFileUsageArgs,

    #[arg(long, short)]
    pub environment: Option<String>,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())?;
    let environment_name = args
        .environment
        .clone()
        .map_or_else(|| EnvironmentName::Default, EnvironmentName::Named);
    let environment = project
        .environment(&environment_name)
        .ok_or_else(|| miette::miette!("unknown environment '{environment_name}'"))?;

    get_up_to_date_prefix(
        &environment,
        args.lock_file_usage.into(),
        false,
        IndexMap::default(),
    )
    .await?;

    if environment
        .task(&TaskName::from("postinstall"), None)
        .is_ok()
    {
        tracing::info!("`postintall` task detected in current environment");

        // Construct arguments to call postinstall task
        let args: crate::cli::run::Args = crate::cli::run::Args {
            task: vec!["postinstall".to_string()],
            manifest_path: args.manifest_path,
            lock_file_usage: args.lock_file_usage,
            environment: args.environment,
        };

        // Execute postinstall task
        crate::cli::run::execute(args).await?;
    }

    // Emit success
    eprintln!(
        "{}Project in {} is ready to use!",
        console::style(console::Emoji("✔ ", "")).green(),
        project.root().display()
    );
    Ok(())
}
