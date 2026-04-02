use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use super::{
    CondaPrefixUpdater,
    resolve::build_dispatch::LazyBuildDispatchDependencies,
    satisfiability::{ValidatedSourceTimestamps, VerifySatisfiabilityContext, pypi_metadata},
    verify_environment_satisfiability, verify_platform_satisfiability,
};
use crate::{
    Workspace,
    lock_file::{
        records_by_name::LockedPypiRecordsByName,
        satisfiability::{EnvironmentUnsat, verify_solve_group_satisfiability},
    },
    workspace::{Environment, SolveGroup},
};
use dashmap::DashMap;
use fancy_display::FancyDisplay;
use futures::StreamExt;
use itertools::Itertools;
use once_cell::sync::OnceCell;
use pixi_command_dispatcher::executor::CancellationAwareFutures;
use pixi_command_dispatcher::{CommandDispatcher, CommandDispatcherError};
use pixi_consts::consts;
use pixi_manifest::{EnvironmentName, FeaturesExt};
use pixi_uv_context::UvResolutionContext;
use rattler_conda_types::Platform;
use rattler_lock::{LockFile, LockedPackageRef};

/// Cache for build-related resources that can be shared between
/// satisfiability checking and PyPI resolution.
#[derive(Default)]
pub struct PypiEnvironmentBuildCache {
    /// Lazily initialized build dispatch dependencies (interpreter, env, etc.)
    pub lazy_build_dispatch_deps: LazyBuildDispatchDependencies,
    /// Optional conda prefix updater (created during satisfiability checking)
    pub conda_prefix_updater: OnceCell<CondaPrefixUpdater>,
}

/// Key for the build cache, combining environment name and platform.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BuildCacheKey {
    pub environment: EnvironmentName,
    pub platform: Platform,
}

impl BuildCacheKey {
    pub fn new(environment: EnvironmentName, platform: Platform) -> Self {
        Self {
            environment,
            platform,
        }
    }
}

/// A struct that contains information about specific outdated environments.
///
/// Use the [`OutdatedEnvironments::from_project_and_lock_file`] to create an
/// instance of this struct by examining the project and lock-file and finding
/// any mismatches.
pub struct OutdatedEnvironments<'p> {
    /// The conda environments that are considered out of date with the
    /// lock-file.
    pub conda: HashMap<Environment<'p>, HashSet<Platform>>,

    /// The pypi environments that are considered out of date with the
    /// lock-file.
    pub pypi: HashMap<Environment<'p>, HashSet<Platform>>,

    /// Records the environments for which the lock-file content should also be
    /// discarded. This is the case for instance when the order of the
    /// channels changed.
    pub disregard_locked_content: DisregardLockedContent<'p>,

    /// Lazily initialized UV context for building dynamic metadata.
    /// This is shared between satisfiability checking and pypi resolution.
    pub uv_context: OnceCell<UvResolutionContext>,

    /// Per-environment-platform build caches for sharing resources between
    /// satisfiability checking and PyPI resolution.
    pub build_caches: HashMap<BuildCacheKey, Arc<PypiEnvironmentBuildCache>>,

    /// Cache for static metadata extracted from pyproject.toml files.
    /// This is shared across platforms since static metadata is platform-independent.
    pub static_metadata_cache: HashMap<PathBuf, pypi_metadata::LocalPackageMetadata>,

    /// Locked pypi records with metadata, resolved during the satisfiability
    /// check. Forwarded to the update path to avoid re-reading source trees.
    pub locked_pypi_records: HashMap<(Environment<'p>, Platform), LockedPypiRecordsByName>,

    /// Source record timestamps that were validated unchanged during
    /// satisfiability checking.
    pub validated_source_timestamps:
        HashMap<(Environment<'p>, Platform), ValidatedSourceTimestamps>,
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
    pub(crate) fn should_disregard_conda(&self, env: &Environment<'p>) -> bool {
        self.conda.contains(env)
    }

    /// Returns true if the pypi locked content should be ignored for the given
    /// environment.
    pub(crate) fn should_disregard_pypi(&self, env: &Environment<'p>) -> bool {
        self.conda.contains(env) || self.pypi.contains(env)
    }
}

impl<'p> OutdatedEnvironments<'p> {
    /// Constructs a new instance of this struct by examining the project and
    /// lock-file and finding any mismatches.
    pub(crate) async fn from_workspace_and_lock_file(
        workspace: &'p Workspace,
        command_dispatcher: CommandDispatcher,
        lock_file: &LockFile,
    ) -> Self {
        // Find all targets that are not satisfied by the lock-file
        let (
            UnsatisfiableTargets {
                mut outdated_conda,
                mut outdated_pypi,
                disregard_locked_content,
            },
            uv_context,
            build_caches,
            static_metadata_cache,
            locked_pypi_records,
            validated_source_timestamps,
        ) = find_unsatisfiable_targets(workspace, command_dispatcher, lock_file).await;

        // Extend the outdated targets to include the solve groups
        let (mut conda_solve_groups_out_of_date, mut pypi_solve_groups_out_of_date) =
            map_outdated_targets_to_solve_groups(&outdated_conda, &outdated_pypi);

        // Find all the solve groups that have inconsistent dependencies between
        // environments.
        find_inconsistent_solve_groups(
            workspace,
            lock_file,
            &outdated_conda,
            &mut conda_solve_groups_out_of_date,
            &mut pypi_solve_groups_out_of_date,
        );

        // Mark the rest of the environments out of date for all solve groups
        for (solve_group, platforms) in conda_solve_groups_out_of_date {
            for env in solve_group.environments() {
                let env_platforms = env.platforms();
                outdated_conda.entry(env.clone()).or_default().extend(
                    platforms
                        .iter()
                        .filter(|p| env_platforms.contains(p))
                        .copied(),
                );
            }
        }

        for (solve_group, platforms) in pypi_solve_groups_out_of_date {
            for env in solve_group.environments() {
                let env_platforms = env.platforms();
                outdated_pypi.entry(env.clone()).or_default().extend(
                    platforms
                        .iter()
                        .filter(|p| env_platforms.contains(p))
                        .copied(),
                );
            }
        }

        // For all targets where conda is out of date, the pypi packages are also out of
        // date.
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
            uv_context,
            build_caches,
            static_metadata_cache,
            locked_pypi_records,
            validated_source_timestamps,
        }
    }

    /// Returns true if the lock-file is up-to-date with the project (e.g. there
    /// are no outdated targets).
    pub(crate) fn is_empty(&self) -> bool {
        self.conda.is_empty() && self.pypi.is_empty()
    }
}

#[derive(Debug, Default)]
struct UnsatisfiableTargets<'p> {
    outdated_conda: HashMap<Environment<'p>, HashSet<Platform>>,
    outdated_pypi: HashMap<Environment<'p>, HashSet<Platform>>,
    disregard_locked_content: DisregardLockedContent<'p>,
}

/// Find all targets (combination of environment and platform) who's
/// requirements in the `project` are not satisfied by the `lock_file`.
///
/// Returns the unsatisfiable targets, the lazily-initialized UV context
/// (which may have been initialized during satisfiability checking),
/// build caches for each environment, and the static metadata cache.
async fn find_unsatisfiable_targets<'p>(
    project: &'p Workspace,
    command_dispatcher: CommandDispatcher,
    lock_file: &LockFile,
) -> (
    UnsatisfiableTargets<'p>,
    OnceCell<UvResolutionContext>,
    HashMap<BuildCacheKey, Arc<PypiEnvironmentBuildCache>>,
    HashMap<PathBuf, pypi_metadata::LocalPackageMetadata>,
    HashMap<(Environment<'p>, Platform), LockedPypiRecordsByName>,
    HashMap<(Environment<'p>, Platform), ValidatedSourceTimestamps>,
) {
    let mut verified_environments = HashMap::new();
    let mut locked_pypi_by_env_platform = HashMap::new();
    let mut validated_source_timestamps = HashMap::new();
    let mut unsatisfiable_targets = UnsatisfiableTargets::default();

    // Create UV context lazily for building dynamic metadata
    let uv_context: OnceCell<UvResolutionContext> = OnceCell::new();

    // Create build caches for sharing between satisfiability and resolution
    let build_caches: DashMap<BuildCacheKey, Arc<PypiEnvironmentBuildCache>> = DashMap::new();

    // Create static metadata cache for sharing across platforms
    let static_metadata_cache: DashMap<PathBuf, pypi_metadata::LocalPackageMetadata> =
        DashMap::new();

    let project_config = project.config();

    for environment in project.environments() {
        let platforms = environment.platforms();

        // Get the locked environment from the environment
        let Some(locked_environment) = lock_file.environment(environment.name().as_str()) else {
            tracing::info!(
                "environment '{0}' is out of date because it does not exist in the lock-file.",
                environment.name().fancy_display()
            );

            unsatisfiable_targets
                .outdated_conda
                .entry(environment.clone())
                .or_default()
                .extend(platforms.iter().copied());

            continue;
        };

        // The locked environment exists, but does it match our project environment?
        if let Err(unsat) = verify_environment_satisfiability(&environment, locked_environment) {
            tracing::info!(
                "environment '{0}' is out of date because {unsat}",
                environment.name().fancy_display()
            );

            unsatisfiable_targets
                .outdated_conda
                .entry(environment.clone())
                .or_default()
                .extend(platforms.iter().copied());

            match unsat {
                EnvironmentUnsat::AdditionalPlatformsInLockFile(platforms) => {
                    // If there are additional platforms in the lock file, then we have to
                    // remove them
                    for platform in platforms {
                        unsatisfiable_targets
                            .outdated_conda
                            .entry(environment.clone())
                            .or_default()
                            .insert(platform);
                    }
                }
                EnvironmentUnsat::ChannelsExtended => {
                    // Channels were only extended (appended) with lower-priority channels.
                    // Due to channel priority semantics, existing packages are still valid
                    // since they came from higher-priority channels. We just need to update
                    // the lock file's channel list without re-solving.
                    // Don't add to disregard_locked_content.
                }
                EnvironmentUnsat::ChannelsMismatch
                | EnvironmentUnsat::InvalidChannel(_)
                | EnvironmentUnsat::ChannelPriorityMismatch { .. }
                | EnvironmentUnsat::SolveStrategyMismatch { .. }
                | EnvironmentUnsat::ExcludeNewerMismatch(..) => {
                    // We cannot trust any of the locked contents.
                    unsatisfiable_targets
                        .disregard_locked_content
                        .conda
                        .insert(environment.clone());
                }

                EnvironmentUnsat::IndexesMismatch(_)
                | EnvironmentUnsat::InvalidDistExtensionInNoBuild(_)
                | EnvironmentUnsat::NoBuildWithNonBinaryPackages(_)
                | EnvironmentUnsat::PypiWheelTagsMismatch { .. }
                | EnvironmentUnsat::PypiPrereleaseModeMismatch { .. } => {
                    // We cannot trust the python part of the locked contents.
                    unsatisfiable_targets
                        .disregard_locked_content
                        .pypi
                        .insert(environment.clone());
                }
            }

            if unsatisfiable_targets
                .disregard_locked_content
                .should_disregard_conda(&environment)
            {
                continue;
            }
        }

        // Verify each individual platform in parallel
        let mut platform_futures = CancellationAwareFutures::new(command_dispatcher.executor());
        for platform in platforms {
            let ctx = VerifySatisfiabilityContext {
                environment: &environment,
                command_dispatcher: command_dispatcher.clone(),
                platform,
                project_root: project.root(),
                uv_context: &uv_context,
                config: project_config,
                project_env_vars: project.env_vars().clone(),
                build_caches: &build_caches,
                static_metadata_cache: &static_metadata_cache,
            };
            platform_futures.push(async move {
                let result = verify_platform_satisfiability(&ctx, locked_environment).await;
                Ok::<_, CommandDispatcherError<std::convert::Infallible>>((platform, result))
            });
        }

        // Collect all platform results
        while let Some(result) = platform_futures.next().await {
            match result {
                Ok((platform, outcome)) => {
                    if !outcome.validated_source_timestamps.is_empty() {
                        validated_source_timestamps.insert(
                            (environment.clone(), platform),
                            outcome.validated_source_timestamps,
                        );
                    }

                    match outcome.result {
                        Ok((verified_env, locked_pypi)) => {
                            verified_environments
                                .insert((environment.clone(), platform), verified_env);
                            locked_pypi_by_env_platform
                                .insert((environment.clone(), platform), locked_pypi);
                        }
                        Err(CommandDispatcherError::Cancelled) => {
                            // Cancellation is handled by CancellationAwareFutures;
                            // remaining platforms will be skipped automatically.
                        }
                        Err(CommandDispatcherError::Failed(unsat)) if unsat.is_pypi_only() => {
                            tracing::info!(
                                "the pypi dependencies of environment '{0}' for platform {platform} are out of date because {unsat}",
                                environment.name().fancy_display()
                            );

                            unsatisfiable_targets
                                .outdated_pypi
                                .entry(environment.clone())
                                .or_default()
                                .insert(platform);
                        }
                        Err(CommandDispatcherError::Failed(unsat)) => {
                            tracing::info!(
                                "the dependencies of environment '{0}' for platform {platform} are out of date because {unsat}",
                                environment.name().fancy_display()
                            );

                            unsatisfiable_targets
                                .outdated_conda
                                .entry(environment.clone())
                                .or_default()
                                .insert(platform);
                        }
                    }
                }
                Err(CommandDispatcherError::Cancelled) => {
                    unreachable!("platform task cannot cancel")
                }
                Err(CommandDispatcherError::Failed(_)) => unreachable!("platform task cannot fail"),
            }
        }
    }

    // Verify grouped environments
    for solve_group in project.solve_groups() {
        'platform: for platform in solve_group.platforms() {
            let mut envs = Vec::with_capacity(solve_group.environments().len());
            for env in solve_group.environments() {
                if let Some(verified_env) = verified_environments.remove(&(env, platform)) {
                    envs.push(verified_env);
                } else {
                    // If the environment is not verified, the solve group will already be outdated.
                    continue 'platform;
                }
            }

            let Err(unsat) = verify_solve_group_satisfiability(envs) else {
                continue;
            };

            tracing::info!(
                "the dependencies of solve group '{0}' for platform {platform} are out of date because {unsat}",
                solve_group.name(),
            );

            for env in solve_group.environments() {
                unsatisfiable_targets
                    .outdated_conda
                    .entry(env.clone())
                    .or_default()
                    .insert(platform);
            }
        }
    }

    // Verify individual environments as if they are solve-groups
    for ((individual_env, platform), verified_env) in verified_environments {
        let Err(unsat) = verify_solve_group_satisfiability([verified_env]) else {
            continue;
        };

        tracing::info!(
            "the dependencies of environment '{0}' for platform {platform} are out of date because {unsat}",
            individual_env.name().fancy_display(),
        );

        unsatisfiable_targets
            .outdated_conda
            .entry(individual_env.clone())
            .or_default()
            .insert(platform);
    }

    (
        unsatisfiable_targets,
        uv_context,
        build_caches.into_iter().collect(),
        static_metadata_cache.into_iter().collect(),
        locked_pypi_by_env_platform,
        validated_source_timestamps,
    )
}

/// Given a mapping of outdated targets, construct a new mapping of all the
/// groups that are out of date.
///
/// If one of the environments in a solve-group is no longer satisfied by the
/// lock-file all the environments in the same solve-group have to be
/// recomputed.
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

/// Given a `project` and `lock_file`, finds all the solve-groups that have
/// inconsistent dependencies between environments.
///
/// All environments in a solve-group must share the same dependencies. This
/// function iterates over solve-groups and checks if the dependencies of all
/// its environments are the same. For each package name, only one candidate is
/// allowed.
fn find_inconsistent_solve_groups<'p>(
    project: &'p Workspace,
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

        // Keep track of the packages by name to check for mismatches between
        // environments.
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

            let lock_platform = locked_env.lock_file().platform(&platform.to_string());
            for package in lock_platform
                .and_then(|p| locked_env.packages(p))
                .into_iter()
                .flatten()
            {
                match package {
                    LockedPackageRef::Conda(pkg) => match conda_packages_by_name.get(pkg.name()) {
                        None => {
                            conda_packages_by_name
                                .insert(pkg.name().clone(), pkg.location().clone());
                        }
                        Some(url) if pkg.location() != url => {
                            conda_package_mismatch = true;
                        }
                        _ => {}
                    },
                    LockedPackageRef::Pypi(pkg) => match pypi_packages_by_name.get(pkg.name()) {
                        None => {
                            pypi_packages_by_name
                                .insert(pkg.name().clone(), pkg.location().clone());
                        }
                        Some(url) if pkg.location() != url => {
                            pypi_package_mismatch = true;
                        }
                        _ => {}
                    },
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
            tracing::info!(
                "the locked conda packages in solve group {} are not consistent for all environments for platform {}",
                consts::SOLVE_GROUP_STYLE.apply_to(solve_group.name()),
                consts::PLATFORM_STYLE.apply_to(platform)
            );
            conda_solve_groups_out_of_date
                .entry(solve_group.clone())
                .or_default()
                .insert(platform);
        }

        if pypi_package_mismatch && !conda_package_mismatch {
            tracing::info!(
                "the locked pypi packages in solve group {} are not consistent for all environments for platform {}",
                consts::SOLVE_GROUP_STYLE.apply_to(solve_group.name()),
                consts::PLATFORM_STYLE.apply_to(platform)
            );
            pypi_solve_groups_out_of_date
                .entry(solve_group.clone())
                .or_default()
                .insert(platform);
        }
    }
}
