mod package_identifier;
mod pypi;
mod pypi_name_mapping;
mod satisfiability;

use crate::{progress, Project};
use futures::TryStreamExt;
use futures::{stream, StreamExt};
use indicatif::ProgressBar;
use itertools::{izip, Itertools};
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{
    GenericVirtualPackage, MatchSpec, PackageName, Platform, RepoDataRecord,
};
use rattler_lock::{
    LockFile, PackageHashes, PypiPackageData, PypiPackageDataRef, PypiPackageEnvironmentData,
};
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_solve::{resolvo, SolverImpl};
use rip::resolve::SDistResolution;
use std::collections::HashMap;
use std::path::Path;
use std::{sync::Arc, time::Duration};

use crate::project::Environment;
pub use satisfiability::lock_file_satisfies_project;

/// A list of conda packages that are locked for a specific platform.
pub type LockedCondaPackages = Vec<RepoDataRecord>;

/// A list of Pypi packages that are locked for a specific platform.
pub type LockedPypiPackages = Vec<(PypiPackageData, PypiPackageEnvironmentData)>;

/// A list of references to conda packages that are locked for a specific platform.
pub type LockedCondaPackagesRef<'p> = &'p LockedCondaPackages;

/// A list of references to pypi packages that are locked for a specific platform.
pub type LockedPypiPackagesRef<'p> = Vec<PypiPackageDataRef<'p>>;

/// A list of conda packages that are locked for all supported platforms.
pub type LockedCondaEnvironment = HashMap<Platform, LockedCondaPackages>;

/// A list of Pypi packages that are locked for all supported platforms.
pub type LockedPypiEnvironment = HashMap<Platform, LockedPypiPackages>;

/// A list of Pypi packages that are locked for all supported platforms.
pub type LockedPypiEnvironmentRef<'p> = HashMap<Platform, LockedPypiPackagesRef<'p>>;

/// Loads the lockfile for the specified project or returns a dummy one if none could be found.
pub async fn load_lock_file(project: &Project) -> miette::Result<LockFile> {
    let lock_file_path = project.lock_file_path();
    if lock_file_path.is_file() {
        // Spawn a background task because loading the file might be IO bound.
        tokio::task::spawn_blocking(move || LockFile::from_path(&lock_file_path).into_diagnostic())
            .await
            .unwrap_or_else(|e| Err(e).into_diagnostic())
    } else {
        Ok(LockFile::default())
    }
}

fn main_progress_bar(num_bars: u64, message: &'static str) -> ProgressBar {
    let multi_progress = progress::global_multi_progress();
    let top_level_progress = multi_progress.add(ProgressBar::new(num_bars));
    top_level_progress.set_style(progress::long_running_progress_style());
    top_level_progress.set_message(message);
    top_level_progress.enable_steady_tick(Duration::from_millis(50));
    top_level_progress
}

fn platform_solve_bars(platforms: impl IntoIterator<Item = Platform>) -> Vec<ProgressBar> {
    platforms
        .into_iter()
        .map(|platform| {
            let pb = progress::global_multi_progress().add(ProgressBar::new(0));
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
        .collect_vec()
}

/// Updates the lock file for conda dependencies for the specified project.
pub async fn update_lock_file_conda(
    environment: &Environment<'_>,
    existing_lock_file: LockedCondaEnvironment,
    repodata: Option<Vec<SparseRepoData>>,
) -> miette::Result<LockedCondaEnvironment> {
    let platforms = environment.platforms();

    // Get the repodata for the project
    let sparse_repo_data: Arc<[_]> = if let Some(sparse_repo_data) = repodata {
        sparse_repo_data
    } else {
        environment.fetch_sparse_repodata().await?
    }
    .into();

    // Construct a progress bar, a main one and one for each platform.
    let _top_level_progress =
        main_progress_bar(platforms.len() as u64, "resolving conda dependencies");
    let solve_bars = platform_solve_bars(platforms.iter().copied());

    let result = stream::iter(platforms.iter().zip(solve_bars.iter().cloned()))
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
                let empty_vec = vec![];
                let result = resolve_platform(
                    environment,
                    existing_lock_file.get(platform).unwrap_or(&empty_vec),
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

                Ok((*platform, result))
            }
        })
        .buffer_unordered(2)
        .try_collect()
        .await;

    // Clear all progress bars
    for bar in solve_bars {
        bar.finish_and_clear();
    }

    result
}

pub async fn update_lock_file_for_pypi(
    environment: &Environment<'_>,
    locked_conda_packages: &LockedCondaEnvironment,
    locked_pypi_packages: LockedPypiEnvironment,
    python_location: Option<&Path>,
    sdist_resolution: SDistResolution,
) -> miette::Result<LockedPypiEnvironment> {
    let platforms = environment.platforms().into_iter().collect_vec();

    // Construct the progress bars
    let _top_level_progress =
        main_progress_bar(platforms.len() as u64, "resolving pypi dependencies");
    let solve_bars = platform_solve_bars(platforms.iter().copied());

    // Extract conda packages per platform.
    let empty_vec = vec![];
    let lock_conda_packages_per_platform = platforms
        .iter()
        .map(|platform| locked_conda_packages.get(platform).unwrap_or(&empty_vec));

    // Extract previous locked pypi packages per platform
    let empty_vec = vec![];
    let locked_pypi_packages_per_platform = platforms
        .iter()
        .map(|platform| locked_pypi_packages.get(platform).unwrap_or(&empty_vec));

    let result = stream::iter(izip!(
        platforms.iter(),
        lock_conda_packages_per_platform,
        locked_pypi_packages_per_platform,
        solve_bars.iter().cloned(),
    ))
    .map(
        |(platform, locked_conda_packages, locked_pypi_packages, pb)| {
            pb.reset_elapsed();
            pb.set_style(
                indicatif::ProgressStyle::with_template(&format!(
                    "  {{spinner:.dim}} {:<9} [{{elapsed_precise}}] {{msg:.dim}}",
                    platform.to_string(),
                ))
                .unwrap(),
            );

            async move {
                let result = resolve_pypi(
                    environment,
                    locked_conda_packages,
                    locked_pypi_packages,
                    *platform,
                    &pb,
                    python_location,
                    sdist_resolution,
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

                Ok((*platform, result))
            }
        },
    )
    // TODO: Hack to ensure we do not encounter file-locking issues in windows, should look at a better solution
    .buffer_unordered(1)
    .try_collect()
    .await;

    // Clear all progress bars
    for bar in solve_bars {
        bar.finish_and_clear();
    }

    result
}

async fn resolve_pypi(
    environment: &Environment<'_>,
    locked_conda_records: &[RepoDataRecord],
    _locked_pypi_records: &[(PypiPackageData, PypiPackageEnvironmentData)],
    platform: Platform,
    pb: &ProgressBar,
    python_location: Option<&Path>,
    sdist_resolution: SDistResolution,
) -> miette::Result<LockedPypiPackages> {
    // Solve python packages
    pb.set_message("resolving pypi dependencies");
    let python_artifacts = pypi::resolve_dependencies(
        environment,
        platform,
        locked_conda_records,
        python_location,
        sdist_resolution,
    )
    .await?;

    // Clear message
    pb.set_message("");

    // Add pip packages
    let mut locked_packages = LockedPypiPackages::with_capacity(python_artifacts.len());
    for python_artifact in python_artifacts {
        let (artifact, metadata) = environment.project()
            .pypi_package_db()?
            // No need for a WheelBuilder here since any builds should have been done during the
            // [`python::resolve_dependencies`] call.
            .get_metadata(&python_artifact.artifacts, None)
            .await
            .expect("failed to get metadata for a package for which we have already fetched metadata during solving.")
            .expect("no metadata for a package for which we have already fetched metadata during solving.");

        let pkg_data = PypiPackageData {
            name: python_artifact.name.to_string(),
            version: python_artifact.version,
            requires_dist: metadata.requires_dist,
            requires_python: metadata.requires_python,
            url: artifact.url.clone(),
            hash: artifact
                .hashes
                .as_ref()
                .and_then(|hash| PackageHashes::from_hashes(None, hash.sha256)),
        };

        let pkg_env = PypiPackageEnvironmentData {
            extras: python_artifact
                .extras
                .into_iter()
                .map(|e| e.as_str().to_string())
                .collect(),
        };

        locked_packages.push((pkg_data, pkg_env));
    }

    Ok(locked_packages)
}

async fn resolve_platform(
    environment: &Environment<'_>,
    existing_lock_file: &LockedCondaPackages,
    sparse_repo_data: Arc<[SparseRepoData]>,
    platform: Platform,
    pb: ProgressBar,
) -> miette::Result<LockedCondaPackages> {
    let dependencies = environment.dependencies(None, Some(platform));
    let match_specs = dependencies
        .iter_specs()
        .map(|(name, constraint)| MatchSpec::from_nameless(constraint.clone(), Some(name.clone())))
        .collect_vec();

    // Extract the package names from the dependencies
    let package_names = dependencies.names().cloned().collect_vec();

    // Get the virtual packages for this platform
    let virtual_packages = environment.virtual_packages(platform);

    // Get the repodata for the current platform and for NoArch
    pb.set_message("loading repodata");
    let available_packages =
        load_sparse_repo_data_async(platform, package_names.clone(), sparse_repo_data).await?;

    // Solve conda packages
    pb.set_message("resolving conda");
    let mut records = resolve_conda_dependencies(
        match_specs,
        virtual_packages,
        // TODO(baszalmstra): We should not need to clone here. We should be able to pass a reference to the data instead.
        existing_lock_file.clone(),
        available_packages,
    )
    .await?;

    // Add purl's for the conda packages that are also available as pypi packages if we need them.
    if environment.has_pypi_dependencies() {
        pypi::amend_pypi_purls(&mut records).await?;
    }

    Ok(records)
}

/// Solves the conda package environment for the given input. This function is async because it
/// spawns a background task for the solver. Since solving is a CPU intensive task we do not want to
/// block the main task.
async fn resolve_conda_dependencies(
    specs: Vec<MatchSpec>,
    virtual_packages: Vec<GenericVirtualPackage>,
    locked_packages: Vec<RepoDataRecord>,
    available_packages: Vec<Vec<RepoDataRecord>>,
) -> miette::Result<LockedCondaPackages> {
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
