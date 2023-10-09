mod python;

use crate::Project;
use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::conda_lock::builder::{LockFileBuilder, LockedPackage, LockedPackages};
use rattler_conda_types::conda_lock::CondaLock;
use rattler_conda_types::{
    conda_lock, MatchSpec, NamelessMatchSpec, PackageName, Platform, RepoDataRecord, Version,
};
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_solve::{resolvo, SolverImpl};
use std::collections::{HashMap, HashSet, VecDeque};
use std::str::FromStr;

/// Loads the lockfile for the specified project or returns a dummy one if none could be found.
pub async fn load_lock_file(project: &Project) -> miette::Result<CondaLock> {
    let lock_file_path = project.lock_file_path();
    tokio::task::spawn_blocking(move || {
        if lock_file_path.is_file() {
            CondaLock::from_path(&lock_file_path).into_diagnostic()
        } else {
            LockFileBuilder::default().build().into_diagnostic()
        }
    })
    .await
    .unwrap_or_else(|e| Err(e).into_diagnostic())
}

/// Returns true if the locked packages match the dependencies in the project.
pub fn lock_file_up_to_date(project: &Project, lock_file: &CondaLock) -> miette::Result<bool> {
    let platforms = project.platforms();

    // If a platform is missing from the lock file the lock file is completely out-of-date.
    if HashSet::<Platform>::from_iter(lock_file.metadata.platforms.iter().copied())
        != HashSet::from_iter(platforms.iter().copied())
    {
        return Ok(false);
    }

    // Check if the channels in the lock file match our current configuration. Note that the order
    // matters here. If channels are added in a different order, the solver might return a different
    // result.
    let channels = project
        .channels()
        .iter()
        .map(|channel| conda_lock::Channel::from(channel.base_url().to_string()))
        .collect_vec();
    if lock_file.metadata.channels.iter().ne(channels.iter()) {
        return Ok(false);
    }

    // For each platform,
    for platform in platforms.iter().cloned() {
        // Check if all dependencies exist in the lock-file.
        let dependencies = project
            .all_dependencies(platform)?
            .into_iter()
            .collect::<VecDeque<_>>();

        // Construct a queue of dependencies that we wanna find in the lock file
        let mut queue = dependencies.clone();

        // Get the virtual packages for the system
        let virtual_packages = project
            .virtual_packages(platform)?
            .into_iter()
            .map(|vpkg| (vpkg.name.clone(), vpkg))
            .collect::<HashMap<_, _>>();

        // Keep track of which dependencies we already found. Since there can always only be one
        // version per named package we can just keep track of the package names.
        let mut seen = dependencies
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<HashSet<_>>();

        while let Some((name, spec)) = queue.pop_back() {
            // Is this a virtual package? And does it match?
            if let Some(vpkg) = virtual_packages.get(&name) {
                if let Some(version_spec) = spec.version {
                    if !version_spec.matches(&vpkg.version) {
                        tracing::info!("found a dependency on virtual package '{}' but the version spec '{}' does not match the expected version of the virtual package '{}'.", name.as_source(), &version_spec, &vpkg.version);
                        return Ok(false);
                    }
                }
                if let Some(build_spec) = spec.build {
                    if !build_spec.matches(&vpkg.build_string) {
                        tracing::info!("found a dependency on virtual package '{}' but the build spec '{}' does not match the expected build of the virtual package '{}'.", name.as_source(), &build_spec, &vpkg.build_string);
                        return Ok(false);
                    }
                }

                // Virtual package matches
                continue;
            }

            // Find the package in the lock-file that matches our dependency.
            let locked_package = lock_file
                .packages_for_platform(platform)
                .find(|locked_package| locked_dependency_satisfies(locked_package, &name, &spec));

            match locked_package {
                None => {
                    // No package found that matches the dependency, the lock file is not in a
                    // consistent state.
                    tracing::info!("failed to find a locked package for '{} {}', assuming the lock file is out of date.", name.as_source(), &spec);
                    return Ok(false);
                }
                Some(package) => {
                    for (depends_name, depends_constraint) in package.dependencies.iter() {
                        if !seen.contains(depends_name) {
                            // Parse the constraint
                            match NamelessMatchSpec::from_str(&depends_constraint.to_string()) {
                                Ok(spec) => {
                                    queue.push_back((depends_name.clone(), spec));
                                    seen.insert(depends_name.clone());
                                }
                                Err(_) => {
                                    tracing::warn!("failed to parse spec '{}', assuming the lock file is corrupt.", depends_constraint);
                                    return Ok(false);
                                }
                            }
                        }
                    }
                }
            }
        }

        // If the number of "seen" dependencies is less than the number of packages for this
        // platform in the first place, there are more packages in the lock file than are used. This
        // means the lock file is also out of date.
        if seen.len() < lock_file.packages_for_platform(platform).count() {
            tracing::info!("there are more packages in the lock-file than required to fulfill all dependency requirements. Assuming the lock file is out of date.");
            return Ok(false);
        }
    }

    Ok(true)
}

/// Returns true if the specified [`conda_lock::LockedDependency`] satisfies the given MatchSpec.
/// TODO: Move this back to rattler.
/// TODO: Make this more elaborate to include all properties of MatchSpec
fn locked_dependency_satisfies(
    locked_package: &conda_lock::LockedDependency,
    name: &PackageName,
    spec: &NamelessMatchSpec,
) -> bool {
    // Check if the name of the package matches
    if locked_package.name != *name {
        return false;
    }

    // Check if the version matches
    if let Some(version_spec) = &spec.version {
        let v = match Version::from_str(&locked_package.version) {
            Err(_) => return false,
            Ok(v) => v,
        };

        if !version_spec.matches(&v) {
            return false;
        }
    }

    // Check if the build string matches
    match (spec.build.as_ref(), &locked_package.build) {
        (Some(build_spec), Some(build)) => {
            if !build_spec.matches(build) {
                return false;
            }
        }
        (Some(_), None) => return false,
        _ => {}
    }

    true
}

/// Updates the lock file for a project.
pub async fn update_lock_file(
    project: &Project,
    existing_lock_file: CondaLock,
    repodata: Option<Vec<SparseRepoData>>,
) -> miette::Result<CondaLock> {
    let platforms = project.platforms();

    // Get the repodata for the project
    let sparse_repo_data = if let Some(sparse_repo_data) = repodata {
        sparse_repo_data
    } else {
        project.fetch_sparse_repodata().await?
    };

    // Construct a conda lock file
    let channels = project
        .channels()
        .iter()
        .map(|channel| conda_lock::Channel::from(channel.base_url().to_string()));

    // Empty match-specs because these differ per platform
    let mut builder = LockFileBuilder::new(channels, platforms.iter().cloned(), vec![]);
    for platform in platforms.iter().cloned() {
        let dependencies = project.all_dependencies(platform)?;
        let match_specs = dependencies
            .iter()
            .map(|(name, constraint)| {
                MatchSpec::from_nameless(constraint.clone(), Some(name.clone()))
            })
            .collect_vec();

        // Extract the package names from the dependencies
        let package_names = dependencies.keys().collect_vec();

        // Get the repodata for the current platform and for NoArch
        let platform_sparse_repo_data = sparse_repo_data.iter().filter(|sparse| {
            sparse.subdir() == platform.as_str() || sparse.subdir() == Platform::NoArch.as_str()
        });

        // Load only records we need for this platform
        let available_packages = SparseRepoData::load_records_recursive(
            platform_sparse_repo_data,
            package_names.into_iter().cloned(),
            None,
        )
        .into_diagnostic()?;

        // Get the virtual packages for this platform
        let virtual_packages = project.virtual_packages(platform)?;

        // Construct a solver task that we can start solving.
        let task = rattler_solve::SolverTask {
            specs: match_specs.clone(),
            available_packages: &available_packages,
            locked_packages: existing_lock_file
                .packages_for_platform(platform)
                .map(RepoDataRecord::try_from)
                .collect::<Result<Vec<_>, _>>()
                .into_diagnostic()?,
            pinned_packages: vec![],
            virtual_packages,
        };

        // Solve the task
        let records = resolvo::Solver.solve(task).into_diagnostic()?;

        // Solve python packages
        let python_artifacts =
            python::resolve_python_dependencies(project, platform, &records).await?;

        dbg!(python_artifacts);

        // Update lock file
        let mut locked_packages = LockedPackages::new(platform);
        for record in records {
            let locked_package = LockedPackage::try_from(record).into_diagnostic()?;
            locked_packages = locked_packages.add_locked_package(locked_package);
        }

        builder = builder.add_locked_packages(locked_packages);
    }

    let conda_lock = builder.build().into_diagnostic()?;

    // Write the conda lock to disk
    conda_lock
        .to_path(&project.lock_file_path())
        .into_diagnostic()?;

    Ok(conda_lock)
}

/// Returns the [`RepoDataRecord`]s for the packages of the current platform from the lock-file.
pub fn get_required_packages(
    lock_file: &CondaLock,
    platform: Platform,
) -> miette::Result<Vec<RepoDataRecord>> {
    lock_file
        .package
        .iter()
        .filter(|pkg| pkg.platform == platform)
        .map(|pkg| pkg.clone().try_into().into_diagnostic())
        .collect()
}
