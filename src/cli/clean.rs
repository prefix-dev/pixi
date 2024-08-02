use crate::Project;
/// Command to clean the parts of your system which are touched by pixi.
use pixi_config;
use pixi_consts::consts;
use pixi_manifest::EnvironmentName;
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use indicatif::ProgressBar;
use miette::IntoDiagnostic;
use pixi_progress::{global_multi_progress, long_running_progress_style};
use std::str::FromStr;

#[derive(Parser, Debug)]
#[clap(group(clap::ArgGroup::new("command")))]
pub enum Command {
    #[clap(name = "cache")]
    Cache(CacheArgs),
}

/// Clean the parts of your system which are touched by pixi.
/// Defaults to cleaning the environments and task cache.
/// Use the `cache` subcommand to clean the cache.
#[derive(Parser, Debug)]
pub struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    /// The path to 'pixi.toml' or 'pyproject.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// The environment directory to remove.
    #[arg(long, short, conflicts_with = "command")]
    pub environment: Option<String>,
}

/// Clean the cache of your system which are touched by pixi.
#[derive(Parser, Debug)]
pub struct CacheArgs {
    /// Clean only the pypi related cache.
    #[arg(long)]
    pub pypi: bool,

    /// Clean only the conda related cache.
    #[arg(long)]
    pub conda: bool,

    /// Answer yes to all questions.
    #[arg(long)]
    pub yes: bool,
    // TODO: Would be amazing to have a --unused flag to clean only the unused cache.
    //       By searching the inode count of the packages and removing based on that.
    // #[arg(long)]
    // pub unused: bool,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    match args.command {
        Some(Command::Cache(args)) => clean_cache(args).await?,
        None => {
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
                remove_folder_with_progress(explicit_env.dir(), true).await?;
                tracing::info!("Skipping removal of task cache and solve group environments for explicit environment '{:?}'", explicit_env.name());
            } else {
                // Remove all pixi related work from the project.
                if !project.environments_dir().starts_with(project.pixi_dir())
                    && project.default_environments_dir().exists()
                {
                    remove_folder_with_progress(project.default_environments_dir(), false).await?;
                    remove_folder_with_progress(
                        project.default_solve_group_environments_dir(),
                        false,
                    )
                    .await?;
                    remove_folder_with_progress(project.task_cache_folder(), false).await?;
                }
                remove_folder_with_progress(project.environments_dir(), true).await?;
                remove_folder_with_progress(project.solve_group_environments_dir(), false).await?;
                remove_folder_with_progress(project.task_cache_folder(), false).await?;
            }

            Project::warn_on_discovered_from_env(args.manifest_path.as_deref())
        }
    }
    Ok(())
}

/// Clean the pixi cache folders.
async fn clean_cache(args: CacheArgs) -> miette::Result<()> {
    let cache_dir = pixi_config::get_cache_dir()?;
    let mut dirs = vec![];

    if args.pypi {
        dirs.push(cache_dir.join(consts::PYPI_CACHE_DIR));
    }
    if args.conda {
        dirs.push(cache_dir.join("pkgs"));
    }
    if dirs.is_empty() && (args.yes || dialoguer::Confirm::new()
                .with_prompt("No cache types specified using the flags.\nDo you really want to remove all cache directories from your machine?")
                .interact_opt()
                .into_diagnostic()?
                .unwrap_or(false))
            {
                dirs.push(cache_dir);
            }

    if dirs.is_empty() {
        eprintln!("{}", console::style("Nothing to remove.").green());
        return Ok(());
    }

    for dir in dirs {
        remove_folder_with_progress(dir, true).await?;
    }
    Ok(())
}

async fn remove_folder_with_progress(
    folder: PathBuf,
    warning_non_existent: bool,
) -> miette::Result<()> {
    if !folder.exists() {
        if warning_non_existent {
            eprintln!(
                "{}",
                console::style(format!("Folder {:?} was already clean.", &folder)).yellow()
            );
        }
        return Ok(());
    }
    let pb = global_multi_progress().add(ProgressBar::new_spinner());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_style(long_running_progress_style());
    pb.set_message(format!(
        "{} {}",
        console::style("Removing").green(),
        folder.clone().display()
    ));

    // Ignore errors
    let result = tokio::fs::remove_dir_all(&folder).await;
    if let Err(e) = result {
        tracing::info!("Failed to remove folder {:?}: {}", folder, e);
    }

    pb.finish_with_message(format!(
        "{} {}",
        console::style("removed").green(),
        folder.display()
    ));
    Ok(())
}
