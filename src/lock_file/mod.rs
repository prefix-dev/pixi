mod package_identifier;
mod pypi;
mod pypi_name_mapping;
mod satisfiability;

use crate::{progress, Project};
use futures::TryStreamExt;
use futures::{stream, StreamExt};
use indicatif::ProgressBar;
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{
    GenericVirtualPackage, MatchSpec, PackageName, Platform, RepoDataRecord,
};
use rattler_lock::{
    builder::{
        CondaLockedDependencyBuilder, LockFileBuilder, LockedPackagesBuilder,
        PypiLockedDependencyBuilder,
    },
    CondaLock, PackageHashes,
};
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_solve::{resolvo, SolverImpl};
use std::{sync::Arc, time::Duration};

pub use satisfiability::lock_file_satisfies_project;

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
    let num_concurrent = if project.has_pypi_dependencies() {
        // HACK: There is a bug in rip that causes a dead-lock when solving multiple environments
        // at the same time. So if there are pypi dependencies we limit the number of concurrent
        // solves to 1.
        1
    } else {
        // By default we solve 2 platforms concurrently. Could probably do more but solving takes
        // a significant amount of memory.
        2
    };
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
            .buffer_unordered(num_concurrent)
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
    let mut records = resolve_conda_dependencies(
        match_specs,
        virtual_packages,
        locked_packages,
        available_packages,
    )
    .await?;

    // Solve python packages
    pb.set_message("resolving python");
    let python_artifacts = pypi::resolve_pypi_dependencies(project, platform, &mut records).await?;

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
            .pypi_package_db()?
            .get_metadata(&python_artifact.artifacts, None)
            .await
            .expect("failed to get metadata for a package for which we have already fetched metadata during solving.")
            .expect("no metadata for a package for which we have already fetched metadata during solving.");

        let locked_package = PypiLockedDependencyBuilder {
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
