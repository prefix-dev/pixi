use crate::project::Project;
use clap::Parser;
use itertools::Itertools;
use rattler_conda_types::conda_lock::builder::{LockedPackage, LockedPackages};
use rattler_conda_types::conda_lock::PackageHashes;
use rattler_conda_types::{
    conda_lock, conda_lock::builder::LockFileBuilder, conda_lock::CondaLock, ChannelConfig,
    MatchSpec, Platform, Version,
};
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_solve::{LibsolvRepoData, SolverBackend};
use std::collections::HashSet;
use std::str::FromStr;

/// Adds a dependency to the project
#[derive(Parser, Debug)]
pub struct Args {}

// TODO: I dont like this command, if it is at all possible it would be so much better when this
//  command is run when needed. E.g. have a cheap way to determine if the environment is up-to-date,
//  if not, update it.
pub async fn execute(args: Args) -> anyhow::Result<()> {
    let project = Project::discover()?;
    let channels = project.channels(&ChannelConfig::default())?;
    let platforms = project.platforms()?;
    let dependencies = project.dependencies()?;

    // Load the lockfile or create a dummy one
    let lock_file_path = project.lock_file_path();
    let lock_file = if lock_file_path.is_file() {
        CondaLock::from_path(&lock_file_path)?
    } else {
        LockFileBuilder::default().build()?
    };

    // Check if the lock file is up to date with the requirements in the project.
    let lock_file_up_to_date = dependencies.iter().all(|match_spec| {
        lock_file
            .package
            .iter()
            .find(|locked_package| locked_dependency_satisfies(*locked_package, match_spec))
            .is_some()
    });

    let lock_file = if !lock_file_up_to_date {
        update_lock_file(&project, lock_file).await?
    } else {
        lock_file
    };

    Ok(())
}

/// Returns true if the specified [`conda_lock::LockedDependency`] satisfies the given match spec.
/// TODO: Move this back to rattler.
/// TODO: Make this more elaborate to include all properties of MatchSpec
fn locked_dependency_satisfies(
    locked_package: &conda_lock::LockedDependency,
    spec: &MatchSpec,
) -> bool {
    // Check if the name of the package matches
    if Some(locked_package.name.as_str()) != spec.name.as_deref() {
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
            if !build_spec.matches(&build) {
                return false;
            }
        }
        (Some(_), None) => return false,
        _ => {}
    }

    true
}

async fn update_lock_file(
    project: &Project,
    _existing_lock_file: CondaLock,
) -> anyhow::Result<CondaLock> {
    let platforms = project.platforms()?;
    let dependencies = project.dependencies()?;

    // Extract the package names from the dependencies
    let package_names = dependencies
        .iter()
        .filter_map(|spec| spec.name.as_deref())
        .collect_vec();

    // Get the repodata for the project
    let sparse_repo_data = project.fetch_sparse_repodata().await?;

    // Construct a conda lock file
    let channels = project
        .channels(&ChannelConfig::default())?
        .into_iter()
        .map(|channel| conda_lock::Channel::from(channel.base_url().to_string()));

    let mut builder = LockFileBuilder::new(channels, platforms.clone(), dependencies.clone());
    for platform in platforms {
        // Get the repodata for the current platform and for NoArch
        let platform_sparse_repo_data = sparse_repo_data.iter().filter(|sparse| {
            sparse.subdir() == platform.as_str() || sparse.subdir() == Platform::NoArch.as_str()
        });

        // Load only records we need for this platform
        let available_packages = SparseRepoData::load_records_recursive(
            platform_sparse_repo_data,
            package_names.iter().copied(),
        )?;

        // Construct a solver task that we can start solving.
        let task = rattler_solve::SolverTask {
            specs: dependencies.clone(),
            available_packages: available_packages
                .iter()
                .map(|records| LibsolvRepoData::from_records(records)),

            // TODO: All these things.
            locked_packages: vec![],
            pinned_packages: vec![],
            virtual_packages: vec![],
        };

        // Solve the task
        let records = rattler_solve::LibsolvBackend.solve(task)?;

        let mut locked_packages = LockedPackages::new(platform);
        for record in records {
            locked_packages = locked_packages.add_locked_package(LockedPackage {
                name: record.package_record.name,
                version: record.package_record.version.to_string(),
                build_string: record.package_record.build.to_string(),
                url: record.url,
                package_hashes: match (record.package_record.sha256, record.package_record.md5) {
                    (Some(sha256), Some(md5)) => PackageHashes::Md5Sha256(md5, sha256),
                    (Some(sha256), None) => PackageHashes::Sha256(sha256),
                    (None, Some(md5)) => PackageHashes::Md5(md5),
                    _ => unreachable!("package without any hash??"),
                },
                dependency_list: Default::default(),
                optional: None,
            });
        }

        builder = builder.add_locked_packages(locked_packages);
    }

    let conda_lock = builder.build()?;

    // Write the conda lock to disk
    conda_lock.to_path(&project.lock_file_path())?;

    Ok(conda_lock)
}
