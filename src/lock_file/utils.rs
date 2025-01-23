use std::sync::Arc;

use ahash::{HashMap, HashSet};
use pixi_manifest::FeaturesExt;
use pixi_record::PixiRecord;
use rattler_conda_types::{MatchSpec, PackageName, ParseStrictness, Platform};
use rattler_lock::{LockFile, LockFileBuilder, LockedPackageRef};
use tokio::sync::Semaphore;

use crate::{
    project::{grouped_environment::GroupedEnvironment, Environment},
    Project,
};

use super::{records_by_name::HasNameVersion, PixiRecordsByName};

/// Wraps a semaphore to limit the number of concurrent IO operations. The
/// wrapper type provides a convenient default implementation.
#[derive(Clone)]
pub struct IoConcurrencyLimit(Arc<Semaphore>);

impl Default for IoConcurrencyLimit {
    fn default() -> Self {
        Self(Arc::new(Semaphore::new(10)))
    }
}

impl From<IoConcurrencyLimit> for Arc<Semaphore> {
    fn from(value: IoConcurrencyLimit) -> Self {
        value.0
    }
}

/// Constructs a new lock-file where some of the packages have been removed
pub fn filter_lock_file<
    'p,
    'lock,
    F: FnMut(&Environment<'p>, Platform, LockedPackageRef<'lock>) -> bool,
>(
    project: &'p Project,
    lock_file: &'lock LockFile,
    mut filter: F,
) -> LockFile {
    let mut builder = LockFileBuilder::new();

    for (environment_name, environment) in lock_file.environments() {
        // Find the environment in the project
        let Some(project_env) = project.environment(environment_name) else {
            continue;
        };

        // Copy the channels
        builder.set_channels(environment_name, environment.channels().to_vec());

        // Copy the indexes
        let indexes = environment.pypi_indexes().cloned().unwrap_or_else(|| {
            GroupedEnvironment::from(project_env.clone())
                .pypi_options()
                .into()
        });
        builder.set_pypi_indexes(environment_name, indexes);

        // Copy all packages that don't need to be relaxed
        for (platform, packages) in environment.packages_by_platform() {
            for package in packages {
                if filter(&project_env, platform, package) {
                    builder.add_package(environment_name, platform, package.into());
                }
            }
        }
    }

    builder.finish()
}

/// Filter out the packages that are not in the filter names
/// but also add all its dependencies iteratively
pub fn filter_pixi_records(
    pixi_records: Arc<PixiRecordsByName>,
    filter_names: &Vec<PackageName>,
) -> Vec<PixiRecord> {
    let mut entire_map = HashMap::default();

    // First save everything in the map
    for record in pixi_records.records.iter() {
        entire_map.insert(record.name().as_normalized(), record.clone());
    }

    let mut result = Vec::new();
    let mut visited = HashSet::default();
    let mut to_process = Vec::new();

    // Add initial filter names to the processing queue
    for name in filter_names {
        to_process.push(name.as_normalized().to_string());
    }

    // Iteratively process the queue
    while let Some(package_name) = to_process.pop() {
        // Skip if already visited
        if !visited.insert(package_name.clone()) {
            continue;
        }

        // If the package exists in the map, process it
        if let Some(record) = entire_map.get(package_name.as_str()) {
            result.push(record.clone());

            // Add dependencies to the processing queue
            for dependency in &record.package_record().depends {
                if let Ok(name) = MatchSpec::from_str(dependency, ParseStrictness::Lenient) {
                    if let Some(dep_name) = name.name {
                        to_process.push(dep_name.as_normalized().to_string());
                    }
                }
            }
        }
    }

    result
}
