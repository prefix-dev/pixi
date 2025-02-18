use crate::WorkspaceLocator;
use pixi_config;
use pixi_consts::consts;
use pixi_manifest::EnvironmentName;
use std::path::PathBuf;
use std::time::Duration;

use crate::cli::cli_config::WorkspaceConfig;
use clap::Parser;
use fancy_display::FancyDisplay;
use fs_err::tokio as tokio_fs;
use indicatif::ProgressBar;
use miette::IntoDiagnostic;
use pixi_progress::{global_multi_progress, long_running_progress_style};
use std::str::FromStr;

/// Command to clean the parts of your system which are touched by pixi.
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
    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    #[command(subcommand)]
    command: Option<Command>,

    /// The environment directory to remove.
    #[arg(long, short, conflicts_with = "command")]
    pub environment: Option<String>,

    /// Only remove the activation cache
    #[arg(long)]
    pub activation_cache: bool,
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

    /// Clean only the mapping cache.
    #[arg(long)]
    pub mapping: bool,

    /// Clean only `exec` cache
    #[arg(long)]
    pub exec: bool,

    /// Clean only the repodata cache.
    #[arg(long)]
    pub repodata: bool,

    /// Clean only the build backend tools cache.
    #[arg(long)]
    pub tool: bool,

    /// Answer yes to all questions.
    #[clap(short = 'y', long = "yes", alias = "assume-yes")]
    assume_yes: bool,
    // TODO: Would be amazing to have a --unused flag to clean only the unused cache.
    //       By searching the inode count of the packages and removing based on that.
    // #[arg(long)]
    // pub unused: bool,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    if let Some(Command::Cache(args)) = args.command {
        clean_cache(args).await?;
        return Ok(());
    }

    let workspace = WorkspaceLocator::for_cli()
        .with_closest_package(false)
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    let explicit_environment = args
        .environment
        .map(|n| EnvironmentName::from_str(n.as_str()))
        .transpose()?
        .map(|n| {
            workspace.environment(&n).ok_or_else(|| {
                miette::miette!(
                    "unknown environment '{n}' in {}",
                    workspace.workspace.provenance.path.display()
                )
            })
        })
        .transpose()?;

    if let Some(explicit_env) = explicit_environment {
        if args.activation_cache {
            remove_file(explicit_env.activation_cache_file_path(), false).await?;
            tracing::info!(
                "Only removing activation cache for explicit environment '{}'",
                explicit_env.name().fancy_display()
            );
        } else {
            remove_folder_with_progress(explicit_env.dir(), true).await?;
            remove_file(explicit_env.activation_cache_file_path(), false).await?;
            tracing::info!("Skipping removal of task cache and solve group environments for explicit environment '{}'", explicit_env.name().fancy_display());
        }
    } else {
        // Remove all pixi related work from the project.
        if !workspace
            .environments_dir()
            .starts_with(workspace.pixi_dir())
            && workspace.default_environments_dir().exists()
        {
            remove_folder_with_progress(workspace.default_environments_dir(), false).await?;
            remove_folder_with_progress(workspace.default_solve_group_environments_dir(), false)
                .await?;
        }
        remove_folder_with_progress(workspace.environments_dir(), true).await?;
        remove_folder_with_progress(workspace.solve_group_environments_dir(), false).await?;
        remove_folder_with_progress(workspace.task_cache_folder(), false).await?;
        remove_folder_with_progress(workspace.activation_env_cache_folder(), false).await?;
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
        dirs.push(cache_dir.join(consts::CONDA_PACKAGE_CACHE_DIR));
    }
    if args.repodata {
        dirs.push(cache_dir.join(consts::CONDA_REPODATA_CACHE_DIR));
    }
    if args.mapping {
        dirs.push(cache_dir.join(consts::CONDA_PYPI_MAPPING_CACHE_DIR));
    }
    if args.exec {
        dirs.push(cache_dir.join(consts::CACHED_ENVS_DIR));
    }
    if args.tool {
        dirs.push(cache_dir.join(consts::CACHED_BUILD_TOOL_ENVS_DIR));
        // TODO: Let's clean deprecated cache directory.
        // This will be removed in a future release.
        dirs.push(cache_dir.join(consts::_CACHED_BUILD_ENVS_DIR));
    }
    if dirs.is_empty() && (args.assume_yes || dialoguer::Confirm::new()
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
    let result = tokio_fs::remove_dir_all(&folder).await;
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

async fn remove_file(file: PathBuf, warning_non_existent: bool) -> miette::Result<()> {
    if !file.exists() {
        if warning_non_existent {
            eprintln!(
                "{}",
                console::style(format!("File {:?} was not found.", &file)).yellow()
            );
        }
        return Ok(());
    }

    // Ignore errors
    let result = tokio_fs::remove_file(&file).await;
    if let Err(e) = result {
        tracing::info!("Failed to remove file {:?}: {}", file, e);
    } else {
        eprintln!("{} {}", console::style("removed").green(), file.display());
    }
    Ok(())
}
