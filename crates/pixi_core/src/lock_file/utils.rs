use std::sync::Arc;

use pixi_manifest::FeaturesExt;
use rattler_conda_types::Platform;
use rattler_lock::{LockFile, LockFileBuilder, LockedPackageRef};
use tokio::sync::Semaphore;

use crate::{
    Workspace,
    workspace::{Environment, grouped_environment::GroupedEnvironment},
};

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
    workspace: &'p Workspace,
    lock_file: &'lock LockFile,
    mut filter: F,
) -> LockFile {
    let mut builder = LockFileBuilder::new();

    for (environment_name, environment) in lock_file.environments() {
        // Find the environment in the project
        let Some(project_env) = workspace.environment(environment_name) else {
            continue;
        };

        // Copy the channels
        builder.set_channels(environment_name, environment.channels().to_vec());
        builder.set_options(environment_name, environment.solve_options().clone());

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
