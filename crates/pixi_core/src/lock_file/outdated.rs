use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use super::{
    CondaPrefixUpdater,
    resolve::build_dispatch::LazyBuildDispatchDependencies,
    satisfiability::{
        VerifySatisfiabilityContext, pypi_metadata, verify_environment_satisfiability,
    },
    verify_platform_satisfiability,
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
use pixi_manifest::{EnvironmentName, FeaturesExt, PixiPlatformName};
use pixi_record::LockFileResolver;
use pixi_uv_context::UvResolutionContext;
use rattler_lock::{LockFile, LockedPackage};

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
    pub platform: PixiPlatformName,
}

impl BuildCacheKey {
    pub fn new(environment: EnvironmentName, platform: PixiPlatformName) -> Self {
        Self {
            environment,
            platform,
        }
    }
}

/// A struct that contains information about specific outdated environments.
///
/// Use [`OutdatedEnvironments::from_workspace_and_lock_file`] to create an
/// instance of this struct by examining the project and lock file and finding
/// any mismatches.
pub struct OutdatedEnvironments<'p> {
    /// The conda environments that are considered out of date with the
    /// lock file.
    pub conda: HashMap<Environment<'p>, HashSet<PixiPlatformName>>,

    /// The pypi environments that are considered out of date with the
    /// lock file.
    pub pypi: HashMap<Environment<'p>, HashSet<PixiPlatformName>>,

    /// Records the environments for which the lock file content should also be
    /// discarded. This is the case for instance when the order of the
    /// channels changed.
    pub disregard_locked_content: DisregardLockedContent<'p>,

    /// The names of environments that are present in the lock-file but no
    /// longer exist in the workspace manifest. These environments should be
    /// removed from the lock-file.
    pub removed_environments: HashSet<String>,

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
    pub locked_pypi_records: HashMap<(Environment<'p>, PixiPlatformName), LockedPypiRecordsByName>,
}

/// Restricts which environments and platforms are verified and re-solved when
/// comparing the workspace against a lock file.
///
/// This is only honored in lockfile-less mode
/// ([`Workspace::is_lockfile_less`]): the machine-local solve cache under
/// `.pixi/` is allowed to be partial, so a command only has to bring the
/// environments it actually uses up-to-date -- and only for the platform that
/// is used on this machine. Environments and platforms outside the scope are
/// neither verified nor re-solved; whatever the cache holds for them is
/// carried through unchanged, and they are brought up-to-date lazily by the
/// first command that needs them. A committed `pixi.lock` must always
/// describe every environment and platform, so in normal mode the scope is
/// ignored.
///
/// Scoping happens before solve-group expansion: when a scoped environment
/// turns out to be outdated, every sibling in its solve group is still
/// re-solved together with it (on the scoped platforms) to keep the group
/// consistent.
#[derive(Debug, Clone, Default)]
pub struct UpdateScope {
    environments: HashMap<EnvironmentName, HashSet<PixiPlatformName>>,
}

impl UpdateScope {
    /// Scopes to the given environments, each restricted to the platform that
    /// install/solve targets on this machine, honoring an explicit
    /// `--platform` override. See [`Self::insert_environment`].
    pub fn from_environments<'a, 'p: 'a>(
        environments: impl IntoIterator<Item = &'a Environment<'p>>,
        override_platform: Option<&PixiPlatformName>,
    ) -> Self {
        let mut scope = Self::default();
        for environment in environments {
            scope.insert_environment(environment, override_platform);
        }
        scope
    }

    /// Adds an environment to the scope, restricted to the platform that
    /// install/solve targets on this machine (or the `override_platform` when
    /// given). An environment without a host-runnable declared platform keeps
    /// its full platform set, so the regular unsupported-platform diagnostics
    /// further down the line stay intact.
    pub fn insert_environment(
        &mut self,
        environment: &Environment<'_>,
        override_platform: Option<&PixiPlatformName>,
    ) {
        let platforms = match environment.named_or_best_declared_platform(override_platform) {
            Some(platform) => std::iter::once(platform.name().clone()).collect(),
            None => environment.platforms(),
        };
        self.insert_environment_with_platforms(environment.name().clone(), platforms);
    }

    /// Adds an environment to the scope with an explicit set of platforms,
    /// merging with any platforms already scoped for it.
    pub fn insert_environment_with_platforms(
        &mut self,
        environment: EnvironmentName,
        platforms: impl IntoIterator<Item = PixiPlatformName>,
    ) {
        self.environments
            .entry(environment)
            .or_default()
            .extend(platforms);
    }

    /// The platforms the given environment is scoped to, or `None` when the
    /// environment is out of scope entirely.
    fn platforms(&self, environment: &EnvironmentName) -> Option<&HashSet<PixiPlatformName>> {
        self.environments.get(environment)
    }
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
    /// lock file and finding any mismatches.
    pub(crate) async fn from_workspace_and_lock_file(
        workspace: &'p Workspace,
        command_dispatcher: CommandDispatcher,
        lock_file: &LockFile,
        resolver: &LockFileResolver,
        scope: Option<&UpdateScope>,
    ) -> Self {
        // Find all targets that are not satisfied by the lock file
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
        ) = find_unsatisfiable_targets(workspace, command_dispatcher, lock_file, resolver, scope)
            .await;

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
                        .cloned(),
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
                        .cloned(),
                );
            }
        }

        // For all targets where conda is out of date, the pypi packages are also out of
        // date.
        for (environment, platforms) in outdated_conda.iter() {
            outdated_pypi
                .entry(environment.clone())
                .or_default()
                .extend(platforms.iter().cloned());
        }

        // Find environments that are present in the lock-file but no longer exist in
        // the workspace manifest. These have to be removed from the lock-file.
        let removed_environments = lock_file
            .environments()
            .map(|(name, _)| name.to_string())
            .filter(|name| workspace.environment(name.as_str()).is_none())
            .inspect(|name| {
                tracing::info!(
                    "environment '{name}' is out of date because it no longer exists in the manifest but is still present in the lock-file.",
                );
            })
            .collect();

        Self {
            conda: outdated_conda,
            pypi: outdated_pypi,
            disregard_locked_content,
            removed_environments,
            uv_context,
            build_caches,
            static_metadata_cache,
            locked_pypi_records,
        }
    }

    /// Returns true if the lock file is up-to-date with the project (e.g. there
    /// are no outdated targets).
    pub(crate) fn is_empty(&self) -> bool {
        self.conda.is_empty() && self.pypi.is_empty() && self.removed_environments.is_empty()
    }
}

#[derive(Debug, Default)]
struct UnsatisfiableTargets<'p> {
    outdated_conda: HashMap<Environment<'p>, HashSet<PixiPlatformName>>,
    outdated_pypi: HashMap<Environment<'p>, HashSet<PixiPlatformName>>,
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
    resolver: &LockFileResolver,
    scope: Option<&UpdateScope>,
) -> (
    UnsatisfiableTargets<'p>,
    OnceCell<UvResolutionContext>,
    HashMap<BuildCacheKey, Arc<PypiEnvironmentBuildCache>>,
    HashMap<PathBuf, pypi_metadata::LocalPackageMetadata>,
    HashMap<(Environment<'p>, PixiPlatformName), LockedPypiRecordsByName>,
) {
    let mut verified_environments = HashMap::new();
    let mut locked_pypi_by_env_platform = HashMap::new();
    let mut unsatisfiable_targets = UnsatisfiableTargets::default();

    // Create UV context lazily for building dynamic metadata
    let uv_context: OnceCell<UvResolutionContext> = OnceCell::new();

    // Create build caches for sharing between satisfiability and resolution
    let build_caches: DashMap<BuildCacheKey, Arc<PypiEnvironmentBuildCache>> = DashMap::new();

    // Create static metadata cache for sharing across platforms
    let static_metadata_cache: DashMap<PathBuf, pypi_metadata::LocalPackageMetadata> =
        DashMap::new();

    let project_config = project.config();

    // First pass: cheap synchronous environment-level checks. Collect the
    // environments whose platforms still need verifying.
    let mut environments_to_verify = Vec::new();
    for environment in project.environments() {
        let mut platforms = environment.platforms();
        if let Some(scope) = scope {
            let Some(scoped_platforms) = scope.platforms(environment.name()) else {
                // Out-of-scope environments are neither verified nor
                // re-solved; the machine-local cache keeps whatever state it
                // holds for them until a command actually needs them.
                continue;
            };
            platforms.retain(|platform| scoped_platforms.contains(platform));
        }

        // Get the locked environment from the environment
        let Some(locked_environment) = lock_file.environment(environment.name().as_str()) else {
            tracing::info!(
                "environment '{0}' is out of date because it does not exist in the lock file.",
                environment.name().fancy_display()
            );

            unsatisfiable_targets
                .outdated_conda
                .entry(environment.clone())
                .or_default()
                .extend(platforms.iter().cloned());

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
                .extend(platforms.iter().cloned());

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
                | EnvironmentUnsat::ExcludeNewerMismatch(..)
                | EnvironmentUnsat::PlatformDefinitionChanged(_) => {
                    // We cannot trust any of the locked contents.
                    // For PlatformDefinitionChanged: the records under the
                    // affected platform were solved under different subdir/VP
                    // assumptions and must be re-derived from scratch.
                    unsatisfiable_targets
                        .disregard_locked_content
                        .conda
                        .insert(environment.clone());
                }

                EnvironmentUnsat::SourceExcludeNewerMismatch(..) => {
                    // Source packages will be re-resolved with updated
                    // timestamps during the update phase. No need to disregard
                    // locked content.
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

        environments_to_verify.push((environment, locked_environment, platforms));
    }

    // Second pass: verify every (environment, platform) pair concurrently.
    // Each pair is independent and IO-bound, and the shared caches are
    // `DashMap`s, so a single batch is safe and overlaps across environments.
    let mut platform_futures = CancellationAwareFutures::new(command_dispatcher.executor());
    for (environment, locked_environment, platforms) in &environments_to_verify {
        for platform in platforms {
            let ctx = VerifySatisfiabilityContext {
                environment,
                command_dispatcher: command_dispatcher.clone(),
                platform: platform.clone(),
                project_root: project.root(),
                uv_context: &uv_context,
                config: project_config,
                project_env_vars: project.env_vars().clone(),
                build_caches: &build_caches,
                static_metadata_cache: &static_metadata_cache,
                resolver,
            };
            let locked_environment = *locked_environment;
            platform_futures.push(async move {
                let result = verify_platform_satisfiability(&ctx, locked_environment).await;
                Ok::<_, CommandDispatcherError<std::convert::Infallible>>((
                    environment,
                    platform.clone(),
                    result,
                ))
            });
        }
    }

    // Collect all platform results
    while let Some(result) = platform_futures.next().await {
        match result {
            Ok((environment, platform, outcome)) => {
                match outcome {
                    Ok((verified_env, locked_pypi)) => {
                        verified_environments
                            .insert((environment.clone(), platform.clone()), verified_env);
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

    // Release the futures' borrows on the shared caches before moving them out.
    drop(platform_futures);

    // Verify grouped environments
    for solve_group in project.solve_groups() {
        'platform: for platform in solve_group.platforms() {
            let mut envs = Vec::with_capacity(solve_group.environments().len());
            for env in solve_group.environments() {
                if let Some(verified_env) = verified_environments.remove(&(env, platform.clone())) {
                    envs.push(verified_env);
                } else {
                    // The environment was not verified: either it is already
                    // outdated (making the whole solve group outdated), or it
                    // fell outside the update scope, in which case the group
                    // consistency check is skipped along with it.
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
                    .insert(platform.clone());
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
    )
}

/// Given a mapping of outdated targets, construct a new mapping of all the
/// groups that are out of date.
///
/// If one of the environments in a solve-group is no longer satisfied by the
/// lock file all the environments in the same solve-group have to be
/// recomputed.
fn map_outdated_targets_to_solve_groups<'p>(
    outdated_conda: &HashMap<Environment<'p>, HashSet<PixiPlatformName>>,
    outdated_pypi: &HashMap<Environment<'p>, HashSet<PixiPlatformName>>,
) -> (
    HashMap<SolveGroup<'p>, HashSet<PixiPlatformName>>,
    HashMap<SolveGroup<'p>, HashSet<PixiPlatformName>>,
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
            .extend(platforms.iter().cloned());
    }

    // For each environment that is out of date, add it to the solve group.
    for (environment, platforms) in outdated_pypi.iter() {
        let Some(solve_group) = environment.solve_group() else {
            continue;
        };
        pypi_solve_groups_out_of_date
            .entry(solve_group)
            .or_insert_with(HashSet::new)
            .extend(platforms.iter().cloned());
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
    outdated_conda: &HashMap<Environment<'p>, HashSet<PixiPlatformName>>,
    conda_solve_groups_out_of_date: &mut HashMap<SolveGroup<'p>, HashSet<PixiPlatformName>>,
    pypi_solve_groups_out_of_date: &mut HashMap<SolveGroup<'p>, HashSet<PixiPlatformName>>,
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
                    LockedPackage::Conda(pkg) => match conda_packages_by_name.get(pkg.name()) {
                        None => {
                            conda_packages_by_name
                                .insert(pkg.name().clone(), pkg.location().clone());
                        }
                        Some(url) if pkg.location() != url => {
                            conda_package_mismatch = true;
                        }
                        _ => {}
                    },
                    LockedPackage::Pypi(pkg) => match pypi_packages_by_name.get(pkg.name()) {
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
                consts::PLATFORM_STYLE.apply_to(&platform)
            );
            conda_solve_groups_out_of_date
                .entry(solve_group.clone())
                .or_default()
                .insert(platform.clone());
        }

        if pypi_package_mismatch && !conda_package_mismatch {
            tracing::info!(
                "the locked pypi packages in solve group {} are not consistent for all environments for platform {}",
                consts::SOLVE_GROUP_STYLE.apply_to(solve_group.name()),
                consts::PLATFORM_STYLE.apply_to(&platform)
            );
            pypi_solve_groups_out_of_date
                .entry(solve_group.clone())
                .or_default()
                .insert(platform);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixi_consts::consts;
    use rattler_conda_types::Platform;

    fn test_workspace(manifest: &str) -> Workspace {
        let temp_dir = tempfile::tempdir().unwrap();
        let manifest_path = temp_dir.path().join(consts::WORKSPACE_MANIFEST);
        Workspace::from_str(&manifest_path, manifest).unwrap()
    }

    /// The scope restricts the check to the requested environments, and each
    /// environment is pinned to the single platform install targets on this
    /// machine (the injected current platform for a lockfile-less manifest).
    #[test]
    fn update_scope_restricts_environments_and_platforms() {
        let workspace = test_workspace(
            r#"
            [workspace]
            channels = []
            platforms = []

            [feature.test.dependencies]

            [environments]
            test = ["test"]
            "#,
        );
        let default_env = workspace.default_environment();

        let scope = UpdateScope::from_environments(std::iter::once(&default_env), None);

        let current = PixiPlatformName::from(Platform::current());
        let scoped_platforms = scope.platforms(default_env.name()).unwrap();
        assert_eq!(
            scoped_platforms.iter().collect::<Vec<_>>(),
            vec![&current],
            "the scoped environment is pinned to the current platform"
        );
        assert!(
            scope
                .platforms(workspace.environment("test").unwrap().name())
                .is_none(),
            "environments that were not requested are out of scope"
        );
    }

    /// An explicit `--platform` override wins over the best-match platform,
    /// and unknown overrides fall back to the environment's full platform set
    /// so the regular membership errors surface later instead of silently
    /// solving nothing.
    #[test]
    fn update_scope_honors_platform_override() {
        let workspace = test_workspace(
            r#"
            [workspace]
            channels = []
            platforms = ["linux-64", "osx-arm64", "win-64"]
            "#,
        );
        let default_env = workspace.default_environment();

        let override_platform = PixiPlatformName::from(Platform::Win64);
        let scope =
            UpdateScope::from_environments(std::iter::once(&default_env), Some(&override_platform));
        let scoped_platforms = scope.platforms(default_env.name()).unwrap();
        assert_eq!(
            scoped_platforms.iter().collect::<Vec<_>>(),
            vec![&override_platform]
        );

        // A platform the environment doesn't declare: keep the full set.
        let unknown = PixiPlatformName::from(Platform::LinuxAarch64);
        let scope = UpdateScope::from_environments(std::iter::once(&default_env), Some(&unknown));
        assert_eq!(
            scope.platforms(default_env.name()).unwrap().len(),
            3,
            "an unknown override keeps the environment's full platform set"
        );
    }
}
