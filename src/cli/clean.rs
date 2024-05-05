/// Command to clean the parts of your system which are touched by pixi.
use crate::{config, EnvironmentName, Project};
use std::str::FromStr;
use std::time::Duration;

use crate::progress::{global_multi_progress, long_running_progress_style};
use clap::Parser;
use indicatif::ProgressBar;
use itertools::Itertools;

#[derive(Parser, Debug)]
pub enum Command {
    /// Add a command to the project
    #[clap(name = "env")]
    Environment(EnvironmentArgs),

    #[clap(name = "cache")]
    Cache(CacheArgs),
}

#[derive(Parser, Debug)]
pub struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}
/// Clean the parts of your system which are touched by pixi.
#[derive(Parser, Debug)]
pub struct EnvironmentArgs {
    /// The path to 'pixi.toml' or 'pyproject.toml'
    #[arg(long)]
    pub manifest_path: Option<std::path::PathBuf>,

    #[arg(long, short)]
    pub environment: Option<String>,
}

/// Clean the parts of your system which are touched by pixi.
#[derive(Parser, Debug)]
pub struct CacheArgs {
    #[arg(long)]
    pub all: bool,
    #[arg(long)]
    pub pypi: bool,
    #[arg(long)]
    pub conda: bool,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let command = args
        .command
        .unwrap_or(Command::Environment(EnvironmentArgs {
            manifest_path: None,
            environment: None,
        }));
    match command {
        Command::Environment(args) => {
            let project = Project::load_or_else_discover(args.manifest_path.as_deref())?; // Extract the passed in environment name.

            let explicit_environment = args
                .environment
                .map(|n| EnvironmentName::from_str(n.as_str()))
                .transpose()?
                .map(|n| {
                    project.environment(&n).ok_or_else(|| {
                        miette::miette!(
                            "unknown environment '{n}' in {}",
                            project
                                .manifest_path()
                                .to_str()
                                .expect("expected to have a manifest_path")
                        )
                    })
                })
                .transpose()?;

            if let Some(explicit_env) = explicit_environment {
                let pb = global_multi_progress().add(ProgressBar::new_spinner());
                pb.enable_steady_tick(Duration::from_millis(100));
                pb.set_style(long_running_progress_style());
                let message = format!(
                    "environment: '{}' from '{}'",
                    explicit_env.name().fancy_display(),
                    explicit_env.dir().display()
                );

                if !explicit_env.dir().exists() {
                    pb.finish_with_message(
                        console::style(format!("{} does not exist", message))
                            .yellow()
                            .to_string(),
                    );
                    return Ok(());
                }
                pb.set_message(format!(
                    "{} {}",
                    console::style("Removing").green(),
                    message
                ));

                // Ignore errors
                let _ = tokio::fs::remove_dir(explicit_env.dir()).await;

                pb.finish_with_message(format!(
                    "{} {}",
                    console::style("removed").green(),
                    message
                ));
            } else {
                let pb = global_multi_progress().add(ProgressBar::new_spinner());
                pb.enable_steady_tick(Duration::from_millis(100));
                pb.set_style(long_running_progress_style());
                let message = format!(
                    "all environments in '{}'",
                    project.environments_dir().display()
                );

                if !project.environments_dir().exists() {
                    pb.finish_with_message(
                        console::style(format!("{} does not exist", message))
                            .yellow()
                            .to_string(),
                    );
                    return Ok(());
                }

                pb.set_message(format!(
                    "{} {}",
                    console::style("Removing").green(),
                    message
                ));

                // Ignore errors
                let _ = tokio::fs::remove_dir_all(project.environments_dir()).await;

                pb.finish_with_message(format!(
                    "{} {}",
                    console::style("removed").green(),
                    message
                ));
            }

            Project::warn_on_discovered_from_env(args.manifest_path.as_deref());
        }
        Command::Cache(args) => {
            let mut dirs = vec![];

            if args.pypi {
                dirs.push(config::get_cache_dir()?.join("pypi"));
            }
            if args.conda {
                dirs.push(config::get_cache_dir()?.join("pkgs"));
            }
            if args.all || dirs.is_empty() {
                dirs.push(config::get_cache_dir()?);
            }

            let pb = global_multi_progress().add(ProgressBar::new_spinner());
            pb.enable_steady_tick(Duration::from_millis(100));
            pb.set_style(long_running_progress_style());
            let message = format!(
                "cache from '{}'",
                dirs.iter()
                    .map(|dir| dir.to_string_lossy().to_string())
                    .join(", ")
            );
            pb.set_message(format!(
                "{} {}",
                console::style("Removing").green(),
                message
            ));

            // Ignore errors
            for dir in dirs {
                if dir.exists() {
                    let _ = tokio::fs::remove_dir_all(&dir).await;
                    pb.finish_with_message(format!(
                        "{} {}",
                        console::style("removed").green(),
                        message
                    ));
                }
                pb.finish_with_message(
                    console::style(format!("{} was already removed", message))
                        .yellow()
                        .to_string(),
                );
            }
        }
    }

    Ok(())
}
