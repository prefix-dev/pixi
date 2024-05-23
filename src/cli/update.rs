use std::{collections::HashSet, path::PathBuf};

use ahash::HashMap;
use clap::Parser;
use indexmap::IndexMap;
use itertools::{Either, Itertools};
use rattler_conda_types::Platform;
use rattler_lock::{LockFile, LockFileBuilder, Package};

use crate::{config::ConfigCli, consts, load_lock_file, lock_file::UpdateContext, Project};

/// Update dependencies as recorded in the local lock file
#[derive(Parser, Debug, Default)]
pub struct Args {
    #[clap(flatten)]
    pub config: ConfigCli,

    /// The path to 'pixi.toml' or 'pyproject.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Don't modify the environment, only modify the lock-file.
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
    #[clap(long = "env", short = 'e')]
    pub environments: Option<Vec<String>>,

    /// The platforms to update. If none is specified, all platforms are
    /// updated.
    #[clap(long = "platform", short = 'p')]
    pub platforms: Option<Vec<Platform>>,
}

/// A distilled version of `UpdateSpecsArgs`.
struct UpdateSpecs {
    packages: Option<HashSet<String>>,
    environments: Option<HashSet<String>>,
    platforms: Option<HashSet<Platform>>,
}

impl UpdateSpecs {
    fn from_args(args: UpdateSpecsArgs) -> Self {
        Self {
            packages: args.packages.map(|args| args.into_iter().collect()),
            environments: args.environments.map(|args| args.into_iter().collect()),
            platforms: args.platforms.map(|args| args.into_iter().collect()),
        }
    }

    /// Returns true if the package should be relaxed.
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

        true
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = args.config;
    let project =
        Project::load_or_else_discover(args.manifest_path.as_deref())?.with_cli_config(config);

    // Load the current lock-file, if any. If none is found, a dummy lock-file is
    // returned.
    let loaded_lock_file = load_lock_file(&project).await?;

    // Unlock dependencies in the lock-file that we want to update.
    let relaxed_lock_file = unlock_packages(&loaded_lock_file, &UpdateSpecs::from_args(args.specs));

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
            "{}Lock-file is up-to-date",
            console::style(console::Emoji("✔ ", "")).green()
        );
    } else {
        diff.print();
    }

    Ok(())
}

/// Constructs a new lock-file where some of the constraints have been removed.
fn unlock_packages(lock_file: &LockFile, specs: &UpdateSpecs) -> LockFile {
    let mut builder = LockFileBuilder::new();

    for (environment_name, environment) in lock_file.environments() {
        // Copy channels and indexes
        builder.set_channels(environment_name, environment.channels().to_vec());
        if let Some(indexes) = environment.pypi_indexes().cloned() {
            builder.set_pypi_indexes(environment_name, indexes);
        }

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

    pub fn is_empty(&self) -> bool {
        self.environment.is_empty()
    }

    // Format the lock-file
    pub fn print(&self) {
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

        for (environment_name, environment) in
            self.environment.iter().sorted_by(|(a, _), (b, _)| a.cmp(b))
        {
            println!(
                "Environment: {}",
                consts::ENVIRONMENT_STYLE.apply_to(environment_name),
            );
            for (platform, packages) in environment {
                println!("  Platform: {}", consts::PLATFORM_STYLE.apply_to(platform));
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
                .for_each(|c| match c {
                    Change::Added(p) => {
                        println!(
                            "    {} {} {}",
                            console::style("+").green(),
                            p.name(),
                            format_package_identifier(p)
                        )
                    }
                    Change::Removed(p) => {
                        println!(
                            "    {} {} {}",
                            console::style("-").red(),
                            p.name(),
                            format_package_identifier(p)
                        )
                    }
                    Change::Changed(previous, current) => {
                        println!(
                            "    {} {} {} -> {}",
                            console::style("~").yellow(),
                            previous.name(),
                            format_package_identifier(previous),
                            format_package_identifier(current)
                        )
                    }
                });
            }
        }
    }
}
