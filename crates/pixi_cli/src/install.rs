use std::fmt::Write;

use clap::Parser;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use pixi_config::ConfigCli;
use pixi_core::{
    UpdateLockFileOptions, WorkspaceLocator,
    environment::{InstallFilter, get_update_lock_file_and_prefixes},
    lock_file::{LockFileDerivedData, PackageFilterNames, ReinstallPackages, UpdateMode},
};

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

    /// Skip installation of specific packages present in the lockfile. This
    /// uses a soft exclusion: the package will be skipped but its dependencies
    /// are installed.
    #[arg(long)]
    pub skip: Option<Vec<String>>,

    /// Skip a package and its entire dependency subtree. This performs a hard
    /// exclusion: the package and its dependencies are not installed unless
    /// reachable from another non-skipped root.
    #[arg(long)]
    pub skip_with_deps: Option<Vec<String>>,

    /// Install and build only these package(s) and their dependencies. Can be
    /// passed multiple times.
    #[arg(long)]
    pub only: Option<Vec<String>>,
}

const SKIP_CUTOFF: usize = 5;

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

    // Build the install filter from CLI args
    let filter = InstallFilter::new()
        .skip_direct(args.skip.clone().unwrap_or_default())
        .skip_with_deps(args.skip_with_deps.clone().unwrap_or_default())
        .target_packages(args.only.clone().unwrap_or_default());

    // Update the prefixes by installing all packages
    let (LockFileDerivedData { lock_file, .. }, _) = get_update_lock_file_and_prefixes(
        &environments,
        UpdateMode::Revalidate,
        UpdateLockFileOptions {
            lock_file_usage: args.lock_file_usage.to_usage(),
            no_install: false,
            max_concurrent_solves: workspace.config().max_concurrent_solves(),
        },
        ReinstallPackages::default(),
        &filter,
    )
    .await?;

    // Message what's installed
    let mut message = console::style(console::Emoji("✔ ", "")).green().to_string();

    let skip_opts = args.skip.is_some()
        || args.skip_with_deps.is_some()
        || args.only.as_ref().is_some_and(|v| !v.is_empty());

    if let Ok(Some(environment)) = environments.iter().at_most_one() {
        write!(
            &mut message,
            "The {} environment has been installed",
            environment.name().fancy_display(),
        )
        .unwrap();

        if skip_opts {
            let platform = environment.best_platform();
            let locked_env = lock_file
                .environment(environment.name().as_str())
                .expect("lock file is missing installed environment");

            let names = PackageFilterNames::new(&filter, locked_env, platform).unwrap_or_default();

            let num_skipped = names.ignored.len();
            let num_retained = names.retained.len();

            // When only is set, also print the number of packages that will be installed
            if args.only.as_ref().is_some_and(|v| !v.is_empty()) {
                write!(&mut message, ", including {} packages", num_retained).unwrap();
            }

            // Create set of unmatched packages, that matches the skip filter
            let (matched, unmatched): (Vec<_>, Vec<_>) = args
                .skip
                .iter()
                .flatten()
                .chain(args.skip_with_deps.iter().flatten())
                .partition(|name| names.ignored.contains(*name));

            if !unmatched.is_empty() {
                tracing::warn!(
                    "The skipped arg(s) '{}' did not match any packages in the lock file",
                    unmatched.into_iter().join(", ")
                );
            }

            if !num_skipped > 0 {
                if num_skipped > 0 && num_skipped < SKIP_CUTOFF {
                    let mut skipped_packages_vec: Vec<_> = names.ignored.into_iter().collect();
                    skipped_packages_vec.sort();

                    write!(
                        &mut message,
                        " excluding '{}'",
                        skipped_packages_vec.join("', '")
                    )
                    .unwrap();
                } else if num_skipped > 0 {
                    let num_matched = matched.len();
                    if num_matched > 0 {
                        write!(
                            &mut message,
                            " excluding '{}' and {} other packages",
                            matched.into_iter().join("', '"),
                            num_skipped
                        )
                        .unwrap()
                    } else {
                        write!(&mut message, " excluding {} other packages", num_skipped).unwrap()
                    }
                } else {
                    write!(
                        &mut message,
                        " no packages were skipped (check if cli args were correct)"
                    )
                    .unwrap();
                }
            }
        }
    } else {
        write!(
            &mut message,
            "The following environments have been installed: {}",
            environments
                .iter()
                .format_with(", ", |e, f| f(&e.name().fancy_display())),
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

    eprintln!("{}.", message);

    Ok(())
}
