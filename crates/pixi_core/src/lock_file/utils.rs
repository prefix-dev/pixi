use std::sync::Arc;

use pixi_manifest::FeaturesExt;
use pixi_record::{LockFileResolver, UnresolvedPixiRecord};
use rattler_conda_types::Platform;
use rattler_lock::{LockFile, LockFileBuilder, LockedPackage};
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

/// Identifies a locked package by name and ecosystem, without committing to
/// an owning representation. Lets [`filter_lock_file`] call its predicate for
/// both top-level packages and the transitive build/host entries inside source
/// records — top-level entries pass the real `LockedPackage`'s name, while
/// transitives pass their conda name without needing a synthesized
/// `LockedPackage`.
#[derive(Clone, Copy)]
pub enum LockedPackageKind<'a> {
    Conda(&'a rattler_conda_types::PackageName),
    Pypi(&'a pep508_rs::PackageName),
}

impl<'a> From<&'a LockedPackage> for LockedPackageKind<'a> {
    fn from(package: &'a LockedPackage) -> Self {
        match package {
            LockedPackage::Conda(p) => LockedPackageKind::Conda(p.name()),
            LockedPackage::Pypi(p) => LockedPackageKind::Pypi(p.name()),
        }
    }
}

/// Constructs a new lock-file where some of the packages have been removed.
///
/// `should_keep` is consulted for every package, both at the top level of an
/// environment and (for conda) for each entry inside a kept source record's
/// `build_packages` / `host_packages`. Returning `false` drops the package; for
/// transitives this strips the entry from the source record so a stale copy
/// cannot silently satisfy the next solve.
///
/// The rebuild routes every kept conda package through [`LockFileResolver`] +
/// [`UnresolvedPixiRecord::into_conda_package_data`] so source records'
/// `build_packages` / `host_packages` are re-registered against fresh
/// `PackageIndex` values in the new builder. Adding the raw `LockedPackage`
/// directly would preserve indices into the old package table, which dangle
/// once any package is dropped.
pub fn filter_lock_file<
    'p,
    'lock,
    F: FnMut(&Environment<'p>, Platform, LockedPackageKind<'_>) -> bool,
>(
    workspace: &'p Workspace,
    lock_file: &'lock LockFile,
    mut should_keep: F,
) -> LockFile {
    let workspace_root = workspace.root();
    let resolver = LockFileResolver::build(lock_file, workspace_root)
        .expect("input lockfile should resolve cleanly");

    let platforms: Vec<rattler_lock::PlatformData> = lock_file
        .platforms()
        .map(|p| rattler_lock::PlatformData {
            name: p.name().clone(),
            subdir: p.subdir(),
            virtual_packages: p.virtual_packages().to_vec(),
        })
        .collect();
    let mut builder = LockFileBuilder::new()
        .with_platforms(platforms)
        .expect("lock file platforms should be unique");

    for (environment_name, environment) in lock_file.environments() {
        let Some(project_env) = workspace.environment(environment_name) else {
            continue;
        };

        builder.set_channels(environment_name, environment.channels().to_vec());
        builder.set_options(environment_name, environment.solve_options().clone());

        let indexes = environment.pypi_indexes().cloned().unwrap_or_else(|| {
            GroupedEnvironment::from(project_env.clone())
                .pypi_options()
                .into()
        });
        builder.set_pypi_indexes(environment_name, indexes);

        for (lock_platform, packages) in environment.packages_by_platform() {
            let platform = lock_platform.subdir();
            let platform_str = platform.to_string();
            for package in packages {
                if !should_keep(&project_env, platform, package.into()) {
                    continue;
                }
                match package {
                    LockedPackage::Conda(_) => {
                        let Some(mut record) = resolver.get_for_package(package) else {
                            // Pointer-identity lookup miss should not happen
                            // for a conda package from the same lockfile; skip
                            // defensively.
                            continue;
                        };
                        if let UnresolvedPixiRecord::Source(arc) = &mut record {
                            let src = Arc::make_mut(arc);
                            src.build_packages.retain(|p| {
                                should_keep(
                                    &project_env,
                                    platform,
                                    LockedPackageKind::Conda(p.name()),
                                )
                            });
                            src.host_packages.retain(|p| {
                                should_keep(
                                    &project_env,
                                    platform,
                                    LockedPackageKind::Conda(p.name()),
                                )
                            });
                        }
                        let data = record.into_conda_package_data(&mut builder, workspace_root);
                        builder
                            .add_conda_package(environment_name, &platform_str, data)
                            .expect("platform was registered");
                    }
                    LockedPackage::Pypi(_) => {
                        builder
                            .add_package(environment_name, &platform_str, package.clone())
                            .expect("platform was registered");
                    }
                }
            }
        }
    }

    builder.finish()
}
