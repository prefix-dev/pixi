use super::{verify_environment_satisfiability, verify_platform_satisfiability};
use crate::lock_file::satisfiability::EnvironmentUnsat;
use crate::project::has_features::HasFeatures;
use crate::{consts, project::Environment, project::SolveGroup, Project};
use itertools::Itertools;
use rattler_conda_types::Platform;
use rattler_lock::{LockFile, Package};
use std::collections::{HashMap, HashSet};

/// A struct that contains information about specific outdated environments.
///
/// Use the [`OutdatedEnvironments::from_project_and_lock_file`] to create an instance of this
/// struct by examining the project and lock-file and finding any mismatches.
#[derive(Debug)]
pub struct OutdatedEnvironments<'p> {
    /// The conda environments that are considered out of date with the lock-file.
    pub conda: HashMap<Environment<'p>, HashSet<Platform>>,

    /// The pypi environments that are considered out of date with the lock-file.
    pub pypi: HashMap<Environment<'p>, HashSet<Platform>>,

    /// Records the environments for which the lock-file content should also be discarded. This is
    /// the case for instance when the order of the channels changed.
    pub disregard_locked_content: DisregardLockedContent<'p>,
}

/// A struct that stores whether the locked content of certain environments
/// should be disregarded.
#[derive(Debug, Default)]
pub struct DisregardLockedContent<'p> {
    conda: HashSet<Environment<'p>>,
    pypi: HashSet<Environment<'p>>,
}

impl<'p> DisregardLockedContent<'p> {
    /// Returns true if the conda locked content should be ignored for the given
    /// environment.
    pub fn should_disregard_conda(&self, env: &Environment<'p>) -> bool {
        self.conda.contains(env)
    }

    /// Returns true if the pypi locked content should be ignored for the given
    /// environment.
    pub fn should_disregard_pypi(&self, env: &Environment<'p>) -> bool {
        self.conda.contains(env) || self.pypi.contains(env)
    }
}

impl<'p> OutdatedEnvironments<'p> {
    /// Constructs a new instance of this struct by examining the project and lock-file and finding
    /// any mismatches.
    pub fn from_project_and_lock_file(project: &'p Project, lock_file: &LockFile) -> Self {
        let mut outdated_conda: HashMap<_, HashSet<_>> = HashMap::new();
        let mut outdated_pypi: HashMap<_, HashSet<_>> = HashMap::new();
        let mut disregard_locked_content = DisregardLockedContent::default();

        // Find all targets that are not satisfied by the lock-file
        find_unsatisfiable_targets(
            project,
            lock_file,
            &mut outdated_conda,
            &mut outdated_pypi,
            &mut disregard_locked_content,
        );

        // Extend the outdated targets to include the solve groups
        let (mut conda_solve_groups_out_of_date, mut pypi_solve_groups_out_of_date) =
            map_outdated_targets_to_solve_groups(&outdated_conda, &outdated_pypi);

        // Find all the solve groups that have inconsistent dependencies between environments.
        find_inconsistent_solve_groups(
            project,
            lock_file,
            &outdated_conda,
            &mut conda_solve_groups_out_of_date,
            &mut pypi_solve_groups_out_of_date,
        );

        // Mark the rest of the environments out of date for all solve groups
        for (solve_group, platforms) in conda_solve_groups_out_of_date {
            for env in solve_group.environments() {
                outdated_conda
                    .entry(env.clone())
                    .or_default()
                    .extend(platforms.iter().copied());
            }
        }

        for (solve_group, platforms) in pypi_solve_groups_out_of_date {
            for env in solve_group.environments() {
                outdated_pypi
                    .entry(env.clone())
                    .or_default()
                    .extend(platforms.iter().copied());
            }
        }

        // For all targets where conda is out of date, the pypi packages are also out of date.
        for (environment, platforms) in outdated_conda.iter() {
            outdated_pypi
                .entry(environment.clone())
                .or_default()
                .extend(platforms.iter().copied());
        }

        Self {
            conda: outdated_conda,
            pypi: outdated_pypi,
            disregard_locked_content,
        }
    }

    /// Returns true if the lock-file is up-to-date with the project (e.g. there are no
    /// outdated targets).
    pub fn is_empty(&self) -> bool {
        self.conda.is_empty() && self.pypi.is_empty()
    }
}

/// Find all targets (combination of environment and platform) who's requirements in the `project`
/// are not satisfied by the `lock_file`.
fn find_unsatisfiable_targets<'p>(
    project: &'p Project,
    lock_file: &LockFile,
    outdated_conda: &mut HashMap<Environment<'p>, HashSet<Platform>>,
    outdated_pypi: &mut HashMap<Environment<'p>, HashSet<Platform>>,
    disregard_locked_content: &mut DisregardLockedContent<'p>,
) {
    for environment in project.environments() {
        let platforms = environment.platforms();

        // Get the locked environment from the environment
        let Some(locked_environment) = lock_file.environment(environment.name().as_str()) else {
            tracing::info!(
                "environment '{0}' is out of date because it does not exist in the lock-file.",
                environment.name().fancy_display()
            );

            outdated_conda
                .entry(environment.clone())
                .or_default()
                .extend(platforms);

            continue;
        };

        // The locked environment exists, but does it match our project environment?
        if let Err(unsat) = verify_environment_satisfiability(&environment, &locked_environment) {
            tracing::info!(
                "environment '{0}' is out of date because {unsat}",
                environment.name().fancy_display()
            );

            outdated_conda
                .entry(environment.clone())
                .or_default()
                .extend(platforms);

            match unsat {
                EnvironmentUnsat::ChannelsMismatch => {
                    // If the channels mismatched we also cannot trust any of the locked content.
                    disregard_locked_content.conda.insert(environment.clone());
                }

                EnvironmentUnsat::IndexesMismatch(_) => {
                    // If the indexes mismatched we also cannot trust any of the locked content.
                    disregard_locked_content.pypi.insert(environment.clone());
                }
            }

            continue;
        }

        // Verify each individual platform
        for platform in platforms {
            match verify_platform_satisfiability(
                &environment,
                &locked_environment,
                platform,
                project.root(),
            ) {
                Ok(_) => {}
                Err(unsat) if unsat.is_pypi_only() => {
                    tracing::info!(
                        "the pypi dependencies of environment '{0}' for platform {platform} are out of date because {unsat}",
                        environment.name().fancy_display()
                    );

                    outdated_pypi
                        .entry(environment.clone())
                        .or_default()
                        .insert(platform);
                }
                Err(unsat) => {
                    tracing::info!(
                        "the dependencies of environment '{0}' for platform {platform} are out of date because {unsat}",
                        environment.name().fancy_display()
                    );

                    outdated_conda
                        .entry(environment.clone())
                        .or_default()
                        .insert(platform);
                }
            }
        }
    }
}

/// Given a mapping of outdated targets, construct a new mapping of all the groups that are out of
/// date.
///
/// If one of the environments in a solve-group is no longer satisfied by the lock-file all the
/// environments in the same solve-group have to be recomputed.
fn map_outdated_targets_to_solve_groups<'p>(
    outdated_conda: &HashMap<Environment<'p>, HashSet<Platform>>,
    outdated_pypi: &HashMap<Environment<'p>, HashSet<Platform>>,
) -> (
    HashMap<SolveGroup<'p>, HashSet<Platform>>,
    HashMap<SolveGroup<'p>, HashSet<Platform>>,
) {
    let mut conda_solve_groups_out_of_date = HashMap::new();
    let mut pypi_solve_groups_out_of_date = HashMap::new();

    // For each environment that is out of date, add it to the solve group.
    for (environment, platforms) in outdated_conda.iter() {
        let Some(solve_group) = environment.solve_group() else {
            continue;
        };
        conda_solve_groups_out_of_date
            .entry(solve_group)
            .or_insert_with(HashSet::new)
            .extend(platforms.iter().copied());
    }

    // For each environment that is out of date, add it to the solve group.
    for (environment, platforms) in outdated_pypi.iter() {
        let Some(solve_group) = environment.solve_group() else {
            continue;
        };
        pypi_solve_groups_out_of_date
            .entry(solve_group)
            .or_insert_with(HashSet::new)
            .extend(platforms.iter().copied());
    }

    (
        conda_solve_groups_out_of_date,
        pypi_solve_groups_out_of_date,
    )
}

/// Given a `project` and `lock_file`, finds all the solve-groups that have inconsistent
/// dependencies between environments.
///
/// All environments in a solve-group must share the same dependencies. This function iterates over
/// solve-groups and checks if the dependencies of all its environments are the same. For each
/// package name, only one candidate is allowed.
fn find_inconsistent_solve_groups<'p>(
    project: &'p Project,
    lock_file: &LockFile,
    outdated_conda: &HashMap<Environment<'p>, HashSet<Platform>>,
    conda_solve_groups_out_of_date: &mut HashMap<SolveGroup<'p>, HashSet<Platform>>,
    pypi_solve_groups_out_of_date: &mut HashMap<SolveGroup<'p>, HashSet<Platform>>,
) {
    let solve_groups = project.solve_groups();
    let solve_groups_and_platforms = solve_groups.iter().flat_map(|solve_group| {
        solve_group
            .environments()
            .flat_map(|env| env.platforms())
            .unique()
            .map(move |platform| (solve_group, platform))
    });

    for (solve_group, platform) in solve_groups_and_platforms {
        // Keep track of if any of the package types are out of date
        let mut conda_package_mismatch = false;
        let mut pypi_package_mismatch = false;

        // Keep track of the packages by name to check for mismatches between environments.
        let mut conda_packages_by_name = HashMap::new();
        let mut pypi_packages_by_name = HashMap::new();

        // Iterate over all environments to compare the packages.
        for env in solve_group.environments() {
            if outdated_conda
                .get(&env)
                .and_then(|p| p.get(&platform))
                .is_some()
            {
                // If the environment is already out-of-date there is no need to check it,
                // because the solve-group is already out-of-date.
                break;
            }

            let Some(locked_env) = lock_file.environment(env.name().as_str()) else {
                // If the environment is missing, we already marked it as out of date.
                continue;
            };

            for package in locked_env.packages(platform).into_iter().flatten() {
                match package {
                    Package::Conda(pkg) => {
                        match conda_packages_by_name.get(&pkg.package_record().name) {
                            None => {
                                conda_packages_by_name
                                    .insert(pkg.package_record().name.clone(), pkg.url().clone());
                            }
                            Some(url) if pkg.url() != url => {
                                conda_package_mismatch = true;
                            }
                            _ => {}
                        }
                    }
                    Package::Pypi(pkg) => {
                        match pypi_packages_by_name.get(&pkg.data().package.name) {
                            None => {
                                pypi_packages_by_name
                                    .insert(pkg.data().package.name.clone(), pkg.url().clone());
                            }
                            Some(url) if pkg.url() != url => {
                                pypi_package_mismatch = true;
                            }
                            _ => {}
                        }
                    }
                }

                // If there is a conda package mismatch there is also a pypi mismatch and we
                // can break early.
                if conda_package_mismatch {
                    pypi_package_mismatch = true;
                    break;
                }
            }

            // If there is a conda package mismatch there is also a pypi mismatch and we can
            // break early.
            if conda_package_mismatch {
                pypi_package_mismatch = true;
                break;
            }
        }

        // If there is a mismatch there is a mismatch for the entire group
        if conda_package_mismatch {
            tracing::info!("the locked conda packages in solve group {} are not consistent for all environments for platform {}",
                        consts::SOLVE_GROUP_STYLE.apply_to(solve_group.name()),
                        consts::PLATFORM_STYLE.apply_to(platform));
            conda_solve_groups_out_of_date
                .entry(solve_group.clone())
                .or_default()
                .insert(platform);
        }

        if pypi_package_mismatch && !conda_package_mismatch {
            tracing::info!("the locked pypi packages in solve group {} are not consistent for all environments for platform {}",
                        consts::SOLVE_GROUP_STYLE.apply_to(solve_group.name()),
                        consts::PLATFORM_STYLE.apply_to(platform));
            pypi_solve_groups_out_of_date
                .entry(solve_group.clone())
                .or_default()
                .insert(platform);
        }
    }
}
