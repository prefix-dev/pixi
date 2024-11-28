use std::{cmp::Ordering, collections::HashSet};

use crate::{
    cli::cli_config::ProjectConfig,
    diff::{LockFileDiff, LockFileJsonDiff},
};
use crate::{
    load_lock_file,
    lock_file::{filter_lock_file, UpdateContext},
    Project,
};
use clap::Parser;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::{Context, IntoDiagnostic, MietteDiagnostic};
use pixi_config::ConfigCli;
use pixi_consts::consts;
use pixi_manifest::EnvironmentName;
use rattler_conda_types::Platform;
use rattler_lock::{LockFile, LockedPackageRef};

/// Update dependencies as recorded in the local lock file
#[derive(Parser, Debug, Default)]
pub struct Args {
    #[clap(flatten)]
    pub config: ConfigCli,

    #[clap(flatten)]
    pub project_config: ProjectConfig,

    /// Don't install the (solve) environments needed for pypi-dependencies
    /// solving.
    #[arg(long)]
    pub no_install: bool,

    /// Don't actually write the lockfile or update any environment.
    #[clap(short = 'n', long)]
    pub dry_run: bool,

    #[clap(flatten)]
    pub specs: UpdateSpecsArgs,

    /// Output the changes in JSON format.
    #[clap(long)]
    pub json: bool,
}

#[derive(Parser, Debug, Default)]
pub struct UpdateSpecsArgs {
    /// The packages to update
    pub packages: Option<Vec<String>>,

    /// The environments to update. If none is specified, all environments are
    /// updated.
    #[clap(long = "environment", short = 'e')]
    pub environments: Option<Vec<EnvironmentName>>,

    /// The platforms to update. If none is specified, all platforms are
    /// updated.
    #[clap(long = "platform", short = 'p')]
    pub platforms: Option<Vec<Platform>>,
}

/// A distilled version of `UpdateSpecsArgs`.
/// TODO: In the future if we want to add `--recursive` this datastructure could
///     be used to store information about recursive packages.
struct UpdateSpecs {
    packages: Option<HashSet<String>>,
    environments: Option<HashSet<EnvironmentName>>,
    platforms: Option<HashSet<Platform>>,
}

impl From<UpdateSpecsArgs> for UpdateSpecs {
    fn from(args: UpdateSpecsArgs) -> Self {
        Self {
            packages: args.packages.map(|args| args.into_iter().collect()),
            environments: args.environments.map(|args| args.into_iter().collect()),
            platforms: args.platforms.map(|args| args.into_iter().collect()),
        }
    }
}

impl UpdateSpecs {
    /// Returns true if the package should be relaxed according to the user
    /// input.
    fn should_relax(
        &self,
        environment_name: &EnvironmentName,
        platform: &Platform,
        package: LockedPackageRef<'_>,
    ) -> bool {
        // Check if the platform is in the list of platforms to update.
        if let Some(platforms) = &self.platforms {
            if !platforms.contains(platform) {
                return false;
            }
        }

        // Check if the environmtent is in the list of environments to update.
        if let Some(environments) = &self.environments {
            if !environments.contains(environment_name) {
                return false;
            }
        }

        // Check if the package is in the list of packages to update.
        if let Some(packages) = &self.packages {
            if !packages.contains(package.name()) {
                return false;
            }
        }

        tracing::debug!(
            "relaxing package: {}, env={}, platform={}",
            package.name(),
            environment_name.fancy_display(),
            consts::PLATFORM_STYLE.apply_to(platform),
        );

        true
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = args.config;
    let project = Project::load_or_else_discover(args.project_config.manifest_path.as_deref())?
        .with_cli_config(config);

    let specs = UpdateSpecs::from(args.specs);

    // If the user specified an environment name, check to see if it exists.
    if let Some(env) = &specs.environments {
        for env in env {
            if project.environment(env).is_none() {
                miette::bail!(
                    "could not find an environment named {}",
                    env.fancy_display()
                )
            }
        }
    }

    // Load the current lock-file, if any. If none is found, a dummy lock-file is
    // returned.
    let loaded_lock_file = load_lock_file(&project).await?;

    // If the user specified a package name, check to see if it is even locked.
    if let Some(packages) = &specs.packages {
        for package in packages {
            ensure_package_exists(&loaded_lock_file, package, &specs)?
        }
    }

    // Unlock dependencies in the lock-file that we want to update.
    let relaxed_lock_file = unlock_packages(&project, &loaded_lock_file, &specs);

    // Update the packages in the lock-file.
    let updated_lock_file = UpdateContext::builder(&project)
        .with_lock_file(relaxed_lock_file.clone())
        .with_no_install(args.no_install)
        .finish()
        .await?
        .update()
        .await?;

    // If we're doing a dry-run, we don't want to write the lock-file.
    if !args.dry_run {
        updated_lock_file.write_to_disk()?;
    }

    // Determine the diff between the old and new lock-file.
    let diff = LockFileDiff::from_lock_files(&loaded_lock_file, &updated_lock_file.lock_file);

    // Format as json?
    if args.json {
        let diff = LockFileDiff::from_lock_files(&loaded_lock_file, &updated_lock_file.lock_file);
        let json_diff = LockFileJsonDiff::new(&project, diff);
        let json = serde_json::to_string_pretty(&json_diff).expect("failed to convert to json");
        println!("{}", json);
    } else if diff.is_empty() {
        eprintln!(
            "{}Lock-file was already up-to-date",
            console::style(console::Emoji("✔ ", "")).green()
        );
    } else {
        diff.print()
            .into_diagnostic()
            .context("failed to print lock-file diff")?;
    }

    Ok(())
}

/// Ensures the existence of the specified package
///
/// # Returns
///
/// Returns `miette::Result` with a descriptive error message
/// if the package does not exist.
fn ensure_package_exists(
    lock_file: &LockFile,
    package_name: &str,
    specs: &UpdateSpecs,
) -> miette::Result<()> {
    let environments = lock_file
        .environments()
        .filter_map(|(name, env)| {
            if let Some(envs) = &specs.environments {
                if !envs.contains(name) {
                    return None;
                }
            }
            Some(env)
        })
        .collect_vec();

    let similar_names = environments
        .iter()
        .flat_map(|env| env.packages_by_platform())
        .filter_map(|(p, packages)| {
            if let Some(platforms) = &specs.platforms {
                if !platforms.contains(&p) {
                    return None;
                }
            }
            Some(packages)
        })
        .flatten()
        .map(|p| p.name().to_string())
        .unique()
        .filter_map(|name| {
            let distance = strsim::jaro(package_name, &name);
            if distance > 0.6 {
                Some((name, distance))
            } else {
                None
            }
        })
        .sorted_by(|(_, a), (_, b)| b.partial_cmp(a).unwrap_or(Ordering::Equal))
        .take(5)
        .map(|(name, _)| name)
        .collect_vec();

    if similar_names.first().map(String::as_str) == Some(package_name) {
        return Ok(());
    }

    let message = format!("could not find a package named '{package_name}'");

    Err(MietteDiagnostic {
        message,
        code: None,
        severity: None,
        help: if !similar_names.is_empty() {
            Some(format!(
                "did you mean '{}'?",
                similar_names.iter().format("', '")
            ))
        } else {
            None
        },
        url: None,
        labels: None,
    }
    .into())
}

/// Constructs a new lock-file where some of the constraints have been removed.
fn unlock_packages(project: &Project, lock_file: &LockFile, specs: &UpdateSpecs) -> LockFile {
    filter_lock_file(project, lock_file, |env, platform, package| {
        !specs.should_relax(env.name(), &platform, package)
    })
}
