use clap::Parser;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use pixi_config::ConfigCli;
use pixi_core::{
    UpdateLockFileOptions, WorkspaceLocator,
    environment::get_update_lock_file_and_prefixes,
    lock_file::{ReinstallPackages, UpdateMode},
};
use std::fmt::Write;

use crate::cli_config::WorkspaceConfig;

/// Install an environment, both updating the lockfile and installing the
/// environment.
///
/// This command installs an environment, if the lockfile is not up-to-date it
/// will be updated.
///
/// `pixi install` only installs one environment at a time,
/// if you have multiple environments you can select the right one with the
/// `--environment` flag. If you don't provide an environment, the `default`
/// environment will be installed.
///
/// If you want to install all environments, you can use the `--all` flag.
///
/// Running `pixi install` is not required before running other commands like
/// `pixi run` or `pixi shell`. These commands will automatically install the
/// environment if it is not already installed.
///
/// You can use `pixi reinstall` to reinstall all environments, one environment
/// or just some packages of an environment.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub project_config: WorkspaceConfig,

    #[clap(flatten)]
    pub lock_file_usage: crate::LockFileUsageConfig,

    /// The environment to install
    #[arg(long, short)]
    pub environment: Option<Vec<String>>,

    #[clap(flatten)]
    pub config: ConfigCli,

    /// Install all environments
    #[arg(long, short, conflicts_with = "environment")]
    pub all: bool,

    /// Skip installation of specific packages present in the lockfile. Requires --frozen.
    /// This can be useful for instance in a Dockerfile to skip local source dependencies when installing dependencies.
    #[arg(long, requires = "frozen")]
    pub skip: Option<Vec<String>>,

    /// Install and build only this package and its dependencies
    #[arg(long)]
    pub package: Option<String>,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.project_config.workspace_locator_start())
        .locate()?
        .with_cli_config(args.config);

    // Install either:
    //
    // 1. specific environments
    // 2. all environments
    // 3. default environment (if no environments are specified)
    let envs = if let Some(envs) = args.environment {
        envs
    } else if args.all {
        workspace
            .environments()
            .iter()
            .map(|env| env.name().to_string())
            .collect()
    } else {
        vec![workspace.default_environment().name().to_string()]
    };

    // Get the environments by name
    let environments = envs
        .into_iter()
        .map(|env| workspace.environment_from_name_or_env_var(Some(env)))
        .collect::<Result<Vec<_>, _>>()?;

    // Update the prefixes by installing all packages
    let (lock_file, _) = get_update_lock_file_and_prefixes(
        &environments,
        UpdateMode::Revalidate,
        UpdateLockFileOptions {
            lock_file_usage: args.lock_file_usage.into(),
            no_install: false,
            max_concurrent_solves: workspace.config().max_concurrent_solves(),
        },
        ReinstallPackages::default(),
        &args.skip.clone().unwrap_or_default(),
    )
    .await?;

    let installed_envs = environments
        .iter()
        .map(|env| env.name())
        .collect::<Vec<_>>();

    // Message what's installed
    let mut message = console::style(console::Emoji("✔ ", "")).green().to_string();

    if installed_envs.len() == 1 {
        write!(
            &mut message,
            "The {} environment has been installed",
            installed_envs[0].fancy_display(),
        )
        .unwrap();
    } else {
        write!(
            &mut message,
            "The following environments have been installed: {}",
            installed_envs.iter().map(|n| n.fancy_display()).join(", "),
        )
        .unwrap();
    }

    if let Ok(Some(path)) = workspace.config().detached_environments().path() {
        write!(
            &mut message,
            " in '{}'",
            console::style(path.display()).bold()
        )
        .unwrap()
    }

    if let Some(skip) = &args.skip {
        let mut all_skipped_packages = std::collections::HashSet::new();
        for env in &environments {
            let skipped_packages = lock_file.get_skipped_package_names(env, skip)?;
            all_skipped_packages.extend(skipped_packages);
        }

        if !all_skipped_packages.is_empty() {
            let mut skipped_packages_vec: Vec<_> = all_skipped_packages.into_iter().collect();
            skipped_packages_vec.sort();
            write!(
                &mut message,
                " excluding '{}'",
                skipped_packages_vec.join("', '")
            )
            .unwrap();
        } else {
            tracing::warn!(
                "No packages were skipped. '{}' did not match any packages in the lockfile.",
                skip.join("', '")
            );
        }
    }

    eprintln!("{}.", message);

    Ok(())
}
