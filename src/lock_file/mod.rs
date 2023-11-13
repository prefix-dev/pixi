mod python;
mod python_name_mapping;

use crate::{progress, Project};
use futures::TryStreamExt;
use futures::{stream, StreamExt};
use indicatif::ProgressBar;
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{
    Channel, GenericVirtualPackage, MatchSpec, NamelessMatchSpec, PackageName, Platform,
    RepoDataRecord, Version,
};
use rattler_lock::{
    builder::{
        CondaLockedDependencyBuilder, LockFileBuilder, LockedPackagesBuilder,
        PipLockedDependencyBuilder,
    },
    CondaLock, LockedDependency, PackageHashes,
};
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_solve::{resolvo, SolverImpl};
use rip::Wheel;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    str::FromStr,
    sync::Arc,
    time::Duration,
};

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

    // TODO: Add support for python dependencies
    if !project.python_dependencies().is_empty() {
        tracing::warn!("Checking if a lock-file is up to date with `python-dependencies` in the mix is not yet implemented.");
        return Ok(false);
    }

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
        .map(|channel| rattler_lock::Channel::from(channel.base_url().to_string()))
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
                    if let Some(conda_package) = package.as_conda() {
                        for spec in conda_package.dependencies.iter() {
                            let Ok(spec) = MatchSpec::from_str(spec) else {
                                tracing::warn!("failed to parse spec '{}', assuming the lock file is corrupt.", spec);
                                return Ok(false);
                            };
                            let (Some(depends_name), spec) = spec.into_nameless() else {
                                // TODO: Should we do something with a matchspec that depends on **all** packages?
                                continue;
                            };

                            if !seen.contains(&depends_name) {
                                queue.push_back((depends_name.clone(), spec));
                                seen.insert(depends_name);
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
fn check_channel_package_url(channel: &str, url: &str) -> bool {
    // Try to parse the channel string into a Channel type
    // If this fails, the error will be propagated using `?`
    let Ok(channel) = Channel::from_str(channel, &Default::default()) else {
        return false;
    };

    // Check if the URL starts with the channel's base URL
    // Return true or false accordingly
    url.starts_with(channel.base_url.as_str())
}

/// Returns true if the specified [`conda_lock::LockedDependency`] satisfies the given MatchSpec.
/// TODO: Move this back to rattler.
/// TODO: Make this more elaborate to include all properties of MatchSpec
fn locked_dependency_satisfies(
    locked_package: &LockedDependency,
    name: &PackageName,
    spec: &NamelessMatchSpec,
) -> bool {
    // Check if the name of the package matches
    if locked_package.name != name.as_normalized() {
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

    if let Some(conda) = locked_package.as_conda() {
        match (spec.build.as_ref(), &conda.build) {
            (Some(build_spec), Some(build)) => {
                if !build_spec.matches(build) {
                    return false;
                }
            }
            (Some(_), None) => return false,
            _ => {}
        }

        if let Some(channel) = &spec.channel {
            if !check_channel_package_url(channel.as_str(), conda.url.as_ref()) {
                return false;
            }
        }
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
    let sparse_repo_data: Arc<[_]> = if let Some(sparse_repo_data) = repodata {
        sparse_repo_data
    } else {
        project.fetch_sparse_repodata().await?
    }
    .into();

    // Construct a progress bar
    let multi_progress = progress::global_multi_progress();
    let top_level_progress = multi_progress.add(ProgressBar::new(platforms.len() as u64));
    top_level_progress.set_style(progress::long_running_progress_style());
    top_level_progress.set_message("solving dependencies");
    top_level_progress.enable_steady_tick(Duration::from_millis(50));

    // Construct a conda lock file
    let channels = project
        .channels()
        .iter()
        .map(|channel| rattler_lock::Channel::from(channel.base_url().to_string()));

    // Create progress bars for each platform
    let solve_bars = platforms
        .iter()
        .map(|platform| {
            let pb =
                progress::global_multi_progress().add(ProgressBar::new(platforms.len() as u64));
            pb.set_style(
                indicatif::ProgressStyle::with_template(&format!(
                    "    {:<9} ..",
                    platform.to_string(),
                ))
                .unwrap(),
            );
            pb.enable_steady_tick(Duration::from_millis(100));
            pb
        })
        .collect_vec();

    // Solve each platform concurrently
    let result: miette::Result<Vec<_>> =
        stream::iter(platforms.iter().zip(solve_bars.iter().cloned()))
            .map(|(platform, pb)| {
                pb.reset_elapsed();
                pb.set_style(
                    indicatif::ProgressStyle::with_template(&format!(
                        "  {{spinner:.dim}} {:<9} [{{elapsed_precise}}] {{msg:.dim}}",
                        platform.to_string(),
                    ))
                    .unwrap(),
                );

                let existing_lock_file = &existing_lock_file;
                let sparse_repo_data = sparse_repo_data.clone();
                async move {
                    let result = resolve_platform(
                        project,
                        existing_lock_file,
                        sparse_repo_data.clone(),
                        *platform,
                        pb.clone(),
                    )
                    .await?;

                    pb.set_style(
                        indicatif::ProgressStyle::with_template(&format!(
                            "  {} {:<9} [{{elapsed_precise}}]",
                            console::style(console::Emoji("✔", "↳")).green(),
                            platform.to_string(),
                        ))
                        .unwrap(),
                    );
                    pb.finish();

                    Ok(result)
                }
            })
            .buffer_unordered(2)
            .try_collect()
            .await;

    for bar in solve_bars {
        bar.finish_and_clear();
    }

    // Collect the result of each individual solve
    let mut builder = LockFileBuilder::new(channels, platforms.iter().cloned(), vec![]);
    for locked_packages in result? {
        builder = builder.add_locked_packages(locked_packages);
    }
    let conda_lock = builder.build().into_diagnostic()?;

    // Write the conda lock to disk
    conda_lock
        .to_path(&project.lock_file_path())
        .into_diagnostic()?;

    Ok(conda_lock)
}

async fn resolve_platform(
    project: &Project,
    existing_lock_file: &CondaLock,
    sparse_repo_data: Arc<[SparseRepoData]>,
    platform: Platform,
    pb: ProgressBar,
) -> miette::Result<LockedPackagesBuilder> {
    let dependencies = project.all_dependencies(platform)?;
    let match_specs = dependencies
        .iter()
        .map(|(name, constraint)| MatchSpec::from_nameless(constraint.clone(), Some(name.clone())))
        .collect_vec();

    // Extract the package names from the dependencies
    let package_names = dependencies.keys().cloned().collect_vec();

    // Get the virtual packages for this platform
    let virtual_packages = project.virtual_packages(platform)?;

    // Get the packages that were contained in the last lock-file. We use these as favored packages
    // for the solver (which is called `locked` for rattler_solve).
    let locked_packages = existing_lock_file
        .get_conda_packages_by_platform(platform)
        .into_diagnostic()
        .context("failed to retrieve the conda packages from the previous lock-file")?;

    // Get the repodata for the current platform and for NoArch
    pb.set_message("loading repodata");
    let available_packages =
        load_sparse_repo_data_async(platform, package_names.clone(), sparse_repo_data).await?;

    // Solve conda packages
    pb.set_message("resolving conda");
    let records = resolve_conda_dependencies(
        match_specs,
        virtual_packages,
        locked_packages,
        available_packages,
    )
    .await?;

    // Solve python packages
    pb.set_message("resolving python");
    let python_artifacts = python::resolve_python_dependencies(project, platform, &records).await?;

    // Clear message
    pb.set_message("");

    // Update lock file
    let mut locked_packages = LockedPackagesBuilder::new(platform);

    // Add conda packages
    for record in records {
        let locked_package = CondaLockedDependencyBuilder::try_from(record).into_diagnostic()?;
        locked_packages.add_locked_package(locked_package);
    }

    // Add pip packages
    for python_artifact in python_artifacts {
        let (artifact, metadata) = project
            .python_package_db()?
            .get_metadata::<Wheel, _>(&python_artifact.artifacts)
            .await
            .expect("failed to get metadata for a package for which we have already fetched metadata during solving.")
            .expect("no metadata for a package for which we have already fetched metadata during solving.");

        let locked_package = PipLockedDependencyBuilder {
            name: python_artifact.name.to_string(),
            version: python_artifact.version.to_string(),
            requires_dist: metadata
                .requires_dist
                .into_iter()
                .map(|r| r.to_string())
                .collect(),
            requires_python: metadata.requires_python.map(|r| r.to_string()),
            extras: python_artifact
                .extras
                .into_iter()
                .map(|e| e.as_str().to_string())
                .collect(),
            url: artifact.url.clone(),
            hash: artifact
                .hashes
                .as_ref()
                .and_then(|hash| PackageHashes::from_hashes(None, hash.sha256)),
            source: None,
            build: None,
        };

        locked_packages.add_locked_package(locked_package)
    }
    Ok(locked_packages)
}

/// Solves the conda package environment for the given input. This function is async because it
/// spawns a background task for the solver. Since solving is a CPU intensive task we do not want to
/// block the main task.
async fn resolve_conda_dependencies(
    specs: Vec<MatchSpec>,
    virtual_packages: Vec<GenericVirtualPackage>,
    locked_packages: Vec<RepoDataRecord>,
    available_packages: Vec<Vec<RepoDataRecord>>,
) -> miette::Result<Vec<RepoDataRecord>> {
    // Construct a solver task that we can start solving.
    let task = rattler_solve::SolverTask {
        specs,
        available_packages: &available_packages,
        locked_packages,
        pinned_packages: vec![],
        virtual_packages,
    };

    // Solve the task
    resolvo::Solver.solve(task).into_diagnostic()
}

/// Load the repodata records for the specified platform and package names in the background. This
/// is a CPU and IO intensive task so we run it in a blocking task to not block the main task.
async fn load_sparse_repo_data_async(
    platform: Platform,
    package_names: Vec<PackageName>,
    sparse_repo_data: Arc<[SparseRepoData]>,
) -> miette::Result<Vec<Vec<RepoDataRecord>>> {
    tokio::task::spawn_blocking(move || {
        let platform_sparse_repo_data = sparse_repo_data.iter().filter(|sparse| {
            sparse.subdir() == platform.as_str() || sparse.subdir() == Platform::NoArch.as_str()
        });

        // Load only records we need for this platform
        SparseRepoData::load_records_recursive(platform_sparse_repo_data, package_names, None)
            .into_diagnostic()
    })
    .await
    .map_err(|e| {
        if let Ok(panic) = e.try_into_panic() {
            std::panic::resume_unwind(panic);
        }
        miette::miette!("the operation was cancelled")
    })?
    .with_context(|| {
        format!(
            "failed to load repodata records for platform '{}'",
            platform.as_str()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_url_channel_match() {
        // Test with a full URL channel
        let channel = "https://repo.prefix.dev/conda-forge";
        let url = "https://repo.prefix.dev/conda-forge/some_package";
        assert!(check_channel_package_url(channel, url));
        // Test with a full URL channel that does not match the URL
        let url = "https://repo.other.dev/conda-forge/some_package";
        assert!(!check_channel_package_url(channel, url));

        // Test with a local path channel
        let channel = "file:///home/rarts/development/staged-recipes/build_artifacts";
        let url =
            "file:///home/rarts/development/staged-recipes/build_artifacts/linux-64/some_package";
        assert!(check_channel_package_url(channel, url));
        let url = "file:///home/beskebob/development/staged-recipes/build_artifacts/linux-64/some_package";
        assert!(!check_channel_package_url(channel, url));
    }

    #[test]
    fn test_channel_name_match() {
        // Test with a channel name that matches a segment in the URL
        let channel = "conda-forge";
        let url = "https://conda.anaconda.org/conda-forge/some_package";
        assert!(check_channel_package_url(channel, url));
        let url = "https://conda.anaconda.org/not-conda-forge/some_package";
        assert!(!check_channel_package_url(channel, url));
        let url = "https://repo.prefix.dev/conda-forge/some_package";
        assert!(!check_channel_package_url(channel, url));

        // Test other parts of the url
        let channel = "conda";
        let url = "https://conda.anaconda.org/conda-forge/some_package";
        assert!(!check_channel_package_url(channel, url));
    }
}
