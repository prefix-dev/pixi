use std::borrow::Cow;
use std::{
    cmp::Ordering,
    collections::HashSet,
    io::{stdout, Write},
    path::PathBuf,
};

use ahash::HashMap;
use clap::Parser;
use indexmap::IndexMap;
use itertools::{Either, Itertools};
use miette::{Context, IntoDiagnostic, MietteDiagnostic};
use rattler_conda_types::Platform;
use rattler_lock::{LockFile, LockFileBuilder, Package};
use tabwriter::TabWriter;

use crate::consts::{CondaEmoji, PypiEmoji};
use crate::project::grouped_environment::GroupedEnvironment;
use crate::{
    config::ConfigCli, consts, load_lock_file, lock_file::UpdateContext, EnvironmentName,
    HasFeatures, Project,
};

/// Update dependencies as recorded in the local lock file
#[derive(Parser, Debug, Default)]
pub struct Args {
    #[clap(flatten)]
    pub config: ConfigCli,

    /// The path to 'pixi.toml' or 'pyproject.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Don't install the (solve) environments needed for pypi-dependencies solving.
    #[arg(long)]
    pub no_install: bool,

    /// Don't actually write the lockfile or update any environment.
    #[clap(short = 'n', long)]
    pub dry_run: bool,

    #[clap(flatten)]
    pub specs: UpdateSpecsArgs,
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
    /// Returns true if the package should be relaxed according to the user input.
    fn should_relax(&self, environment_name: &str, platform: Platform, package: &Package) -> bool {
        // Check if the platform is in the list of platforms to update.
        if let Some(platforms) = &self.platforms {
            if !platforms.contains(&platform) {
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
            if !packages.contains(&*package.name()) {
                return false;
            }
        }

        tracing::debug!(
            "relaxing package: {}, env={}, platform={}",
            package.name(),
            consts::ENVIRONMENT_STYLE.apply_to(environment_name),
            consts::PLATFORM_STYLE.apply_to(platform),
        );

        true
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = args.config;
    let project =
        Project::load_or_else_discover(args.manifest_path.as_deref())?.with_cli_config(config);

    let specs = UpdateSpecs::from(args.specs);

    // If the user specified an environment name, check to see if it exists.
    if let Some(env) = &specs.environments {
        for env in env {
            if project.environment(env).is_none() {
                miette::bail!("could not find an environment named '{}'", env)
            }
        }
    }

    // Load the current lock-file, if any. If none is found, a dummy lock-file is
    // returned.
    let loaded_lock_file = load_lock_file(&project).await?;

    // If the user specified a package name, check to see if it is even locked.
    if let Some(packages) = &specs.packages {
        for package in packages {
            check_package_exists(&loaded_lock_file, package, &specs)?
        }
    }

    // Unlock dependencies in the lock-file that we want to update.
    let relaxed_lock_file = unlock_packages(&project, &loaded_lock_file, &specs);

    // Update the packages in the lock-file.
    let updated_lock_file = UpdateContext::builder(&project)
        .with_lock_file(relaxed_lock_file.clone())
        .with_no_install(args.no_install)
        .finish()?
        .update()
        .await?;

    // If we're doing a dry-run, we don't want to write the lock-file.
    if !args.dry_run {
        updated_lock_file.write_to_disk()?;
    }

    // Determine the diff between the old and new lock-file.
    let diff = LockFileDiff::from_lock_files(&loaded_lock_file, &updated_lock_file.lock_file);
    if diff.is_empty() {
        println!(
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

/// Checks if the specified package exists and returns a helpful error message
/// if it doesn't.
fn check_package_exists(
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
    let mut builder = LockFileBuilder::new();

    for (environment_name, environment) in lock_file.environments() {
        // Find the environment in the project
        let Some(project_env) = project.environment(environment_name) else {
            continue;
        };

        // Copy the channels
        builder.set_channels(environment_name, environment.channels().to_vec());

        // Copy the indexes
        let indexes = environment
            .pypi_indexes()
            .cloned()
            .unwrap_or_else(|| GroupedEnvironment::from(project_env).pypi_options().into());
        builder.set_pypi_indexes(environment_name, indexes);

        // Copy all packages that don't need to be relaxed
        for (platform, packages) in environment.packages_by_platform() {
            for package in packages {
                if !specs.should_relax(environment_name, platform, &package) {
                    builder.add_package(environment_name, platform, package);
                }
            }
        }
    }

    builder.finish()
}

// Represents the differences between two sets of packages.
#[derive(Default, Clone)]
pub struct PackagesDiff {
    pub added: Vec<rattler_lock::Package>,
    pub removed: Vec<rattler_lock::Package>,
    pub changed: Vec<(rattler_lock::Package, rattler_lock::Package)>,
}

impl PackagesDiff {
    /// Returns true if the diff is empty.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// Contains the changes between two lock-files.
pub struct LockFileDiff {
    pub environment: IndexMap<String, IndexMap<Platform, PackagesDiff>>,
}

impl LockFileDiff {
    /// Determine the difference between two lock-files.
    pub fn from_lock_files(previous: &LockFile, current: &LockFile) -> Self {
        let mut result = Self {
            environment: IndexMap::new(),
        };

        for (environment_name, environment) in current.environments() {
            let previous = previous.environment(environment_name);

            let mut environment_diff = IndexMap::new();

            for (platform, packages) in environment.packages_by_platform() {
                // Determine the packages that were previously there.
                let (mut previous_conda_packages, mut previous_pypi_packages): (
                    HashMap<_, _>,
                    HashMap<_, _>,
                ) = previous
                    .as_ref()
                    .and_then(|e| e.packages(platform))
                    .into_iter()
                    .flatten()
                    .partition_map(|p| match p {
                        rattler_lock::Package::Conda(p) => {
                            Either::Left((p.package_record().name.clone(), p))
                        }
                        rattler_lock::Package::Pypi(p) => {
                            Either::Right((p.data().package.name.clone(), p))
                        }
                    });

                let mut diff = PackagesDiff::default();

                // Find new and changed packages
                for package in packages {
                    match package {
                        Package::Conda(p) => {
                            let name = &p.package_record().name;
                            match previous_conda_packages.remove(name) {
                                Some(previous) if previous.url() != p.url() => {
                                    diff.changed
                                        .push((Package::Conda(previous), Package::Conda(p)));
                                }
                                None => {
                                    diff.added.push(Package::Conda(p));
                                }
                                _ => {}
                            }
                        }
                        Package::Pypi(p) => {
                            let name = &p.data().package.name;
                            match previous_pypi_packages.remove(name) {
                                Some(previous) if previous.url() != p.url() => {
                                    diff.changed
                                        .push((Package::Pypi(previous), Package::Pypi(p)));
                                }
                                None => {
                                    diff.added.push(Package::Pypi(p));
                                }
                                _ => {}
                            }
                        }
                    }
                }

                // Determine packages that were removed
                for (_, p) in previous_conda_packages {
                    diff.removed.push(Package::Conda(p));
                }
                for (_, p) in previous_pypi_packages {
                    diff.removed.push(Package::Pypi(p));
                }

                environment_diff.insert(platform, diff);
            }

            // Find platforms that were completely removed
            for (platform, packages) in previous
                .as_ref()
                .map(|e| e.packages_by_platform())
                .into_iter()
                .flatten()
                .filter(|(platform, _)| !environment_diff.contains_key(platform))
                .collect_vec()
            {
                let mut diff = PackagesDiff::default();
                for package in packages {
                    match package {
                        Package::Conda(p) => {
                            diff.removed.push(Package::Conda(p));
                        }
                        Package::Pypi(p) => {
                            diff.removed.push(Package::Pypi(p));
                        }
                    }
                }
                environment_diff.insert(platform, diff);
            }

            // Remove empty diffs
            environment_diff.retain(|_, diff| !diff.is_empty());

            result
                .environment
                .insert(environment_name.to_string(), environment_diff);
        }

        // Find environments that were completely removed
        for (environment_name, environment) in previous
            .environments()
            .filter(|(name, _)| !result.environment.contains_key(*name))
            .collect_vec()
        {
            let mut environment_diff = IndexMap::new();
            for (platform, packages) in environment.packages_by_platform() {
                let mut diff = PackagesDiff::default();
                for package in packages {
                    match package {
                        Package::Conda(p) => {
                            diff.removed.push(Package::Conda(p));
                        }
                        Package::Pypi(p) => {
                            diff.removed.push(Package::Pypi(p));
                        }
                    }
                }
                environment_diff.insert(platform, diff);
            }
            result
                .environment
                .insert(environment_name.to_string(), environment_diff);
        }

        // Remove empty environments
        result.environment.retain(|_, diff| !diff.is_empty());

        result
    }

    /// Returns true if the diff is empty.
    pub fn is_empty(&self) -> bool {
        self.environment.is_empty()
    }

    // Format the lock-file diff.
    pub fn print(&self) -> std::io::Result<()> {
        let mut writer = TabWriter::new(stdout());
        for (idx, (environment_name, environment)) in self
            .environment
            .iter()
            .sorted_by(|(a, _), (b, _)| a.cmp(b))
            .enumerate()
        {
            // Find the changes that happened in all platforms.
            let changes_by_platform = environment
                .into_iter()
                .map(|(platform, packages)| {
                    let changes = Self::format_changes(packages)
                        .into_iter()
                        .collect::<HashSet<_>>();
                    (platform, changes)
                })
                .collect::<Vec<_>>();

            // Find the changes that happened in all platforms.
            let common_changes = changes_by_platform
                .iter()
                .fold(None, |acc, (_, changes)| match acc {
                    None => Some(changes.clone()),
                    Some(acc) => Some(acc.intersection(changes).cloned().collect()),
                })
                .unwrap_or_default();

            // Add a new line between environments
            if idx > 0 {
                writeln!(writer, "\t\t\t",)?;
            }

            writeln!(
                writer,
                "{}: {}\t\t\t",
                console::style("Environment").underlined(),
                consts::ENVIRONMENT_STYLE.apply_to(environment_name)
            )?;

            // Print the common changes.
            for (_, line) in common_changes.iter().sorted_by_key(|(name, _)| name) {
                writeln!(writer, "  {}", line)?;
            }

            // Print the per-platform changes.
            for (platform, changes) in changes_by_platform {
                let mut changes = changes
                    .iter()
                    .filter(|change| !common_changes.contains(change))
                    .sorted_by_key(|(name, _)| name)
                    .peekable();
                if changes.peek().is_some() {
                    writeln!(
                        writer,
                        "{}: {}:{}\t\t\t",
                        console::style("Platform").underlined(),
                        consts::ENVIRONMENT_STYLE.apply_to(environment_name),
                        consts::PLATFORM_STYLE.apply_to(platform),
                    )?;
                    for (_, line) in changes {
                        writeln!(writer, "  {}", line)?;
                    }
                }
            }
        }

        writer.flush()?;

        Ok(())
    }

    fn format_changes(packages: &PackagesDiff) -> Vec<(Cow<'_, str>, String)> {
        enum Change<'i> {
            Added(&'i Package),
            Removed(&'i Package),
            Changed(&'i Package, &'i Package),
        }

        fn format_package_identifier(package: &Package) -> String {
            match package {
                Package::Conda(p) => format!(
                    "{} {}",
                    &p.package_record().version.as_str(),
                    &p.package_record().build
                ),
                Package::Pypi(p) => p.data().package.version.to_string(),
            }
        }

        itertools::chain!(
            packages.added.iter().map(Change::Added),
            packages.removed.iter().map(Change::Removed),
            packages.changed.iter().map(|a| Change::Changed(&a.0, &a.1))
        )
        .sorted_by_key(|c| match c {
            Change::Added(p) => p.name(),
            Change::Removed(p) => p.name(),
            Change::Changed(p, _) => p.name(),
        })
        .map(|p| match p {
            Change::Added(p) => (
                p.name(),
                format!(
                    "{} {} {}\t{}\t\t",
                    console::style("+").green(),
                    match p {
                        Package::Conda(_) => CondaEmoji.to_string(),
                        Package::Pypi(_) => PypiEmoji.to_string(),
                    },
                    p.name(),
                    format_package_identifier(p)
                ),
            ),
            Change::Removed(p) => (
                p.name(),
                format!(
                    "{} {} {}\t{}\t\t",
                    console::style("-").red(),
                    match p {
                        Package::Conda(_) => CondaEmoji.to_string(),
                        Package::Pypi(_) => PypiEmoji.to_string(),
                    },
                    p.name(),
                    format_package_identifier(p)
                ),
            ),
            Change::Changed(previous, current) => {
                fn choose_style<'a>(a: &'a str, b: &'a str) -> console::StyledObject<&'a str> {
                    if a == b {
                        console::style(a).dim()
                    } else {
                        console::style(a)
                    }
                }

                let name = previous.name();
                let line = match (previous, current) {
                    (Package::Conda(previous), Package::Conda(current)) => {
                        let previous = previous.package_record();
                        let current = current.package_record();

                        format!(
                            "{} {} {}\t{} {}\t->\t{} {}",
                            console::style("~").yellow(),
                            CondaEmoji,
                            name,
                            choose_style(&previous.version.as_str(), &current.version.as_str()),
                            choose_style(previous.build.as_str(), current.build.as_str()),
                            choose_style(&current.version.as_str(), &previous.version.as_str()),
                            choose_style(current.build.as_str(), previous.build.as_str()),
                        )
                    }
                    (Package::Pypi(previous), Package::Pypi(current)) => {
                        let previous = previous.data().package;
                        let current = current.data().package;

                        format!(
                            "{} {} {}\t{}\t->\t{}",
                            console::style("~").yellow(),
                            PypiEmoji,
                            name,
                            choose_style(
                                &previous.version.to_string(),
                                &current.version.to_string()
                            ),
                            choose_style(
                                &current.version.to_string(),
                                &previous.version.to_string()
                            ),
                        )
                    }
                    _ => unreachable!(),
                };

                (name, line)
            }
        })
        .collect()
    }
}
