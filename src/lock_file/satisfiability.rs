use super::package_identifier;
use crate::project::{DependencyKind, DependencyName};
use crate::{
    lock_file::pypi::{determine_marker_environment, is_python_record},
    Project,
};
use itertools::Itertools;
use miette::IntoDiagnostic;
use pep508_rs::Requirement;
use rattler_conda_types::{MatchSpec, Platform, Version};
use rattler_lock::{CondaLock, LockedDependency, LockedDependencyKind};
use rip::types::NormalizedPackageName;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    str::FromStr,
};

/// Returns true if the locked packages match the dependencies in the project.
pub fn lock_file_satisfies_project(
    project: &Project,
    lock_file: &CondaLock,
) -> miette::Result<bool> {
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
        .into_iter()
        .map(|channel| rattler_lock::Channel::from(channel.base_url().to_string()))
        .collect_vec();
    if lock_file.metadata.channels.iter().ne(channels.iter()) {
        return Ok(false);
    }

    // For each platform,
    for platform in platforms.iter().cloned() {
        // Check if all dependencies exist in the lock-file.
        let conda_dependencies = project
            .all_dependencies(Some(platform))
            .iter_specs()
            .map(|(name, spec)| {
                DependencyKind::Conda(MatchSpec::from_nameless(spec.clone(), Some(name.clone())))
            })
            .collect::<Vec<_>>();

        let mut pypi_dependencies = project
            .pypi_dependencies(platform)
            .into_iter()
            .map(|(name, requirement)| requirement.as_pep508(&name))
            .map(DependencyKind::PyPi)
            .peekable();

        // Determine the python marker environment from the lock-file.
        let python_marker_env = if pypi_dependencies.peek().is_some() {
            // Determine the python executable
            let Ok(conda_packages) = lock_file.get_conda_packages_by_platform(platform) else {
                tracing::info!("failed to convert conda package to RepoDataRecord, assuming the lockfile is corrupt.");
                return Ok(false);
            };

            // Find the python package
            let Some(python_record) = conda_packages.into_iter().find(is_python_record) else {
                tracing::info!(
                    "there are pypi-dependencies but there is no python version in the lock-file"
                );
                return Ok(false);
            };

            // Construct the marker environment
            let marker_environment =
                match determine_marker_environment(platform, &python_record.package_record) {
                    Ok(marker_environment) => marker_environment,
                    Err(e) => {
                        tracing::info!(
                            "failed to determine marker environment from the lock-file: {e}"
                        );
                        return Ok(false);
                    }
                };

            Some(marker_environment)
        } else {
            None
        };

        let dependencies = conda_dependencies
            .into_iter()
            .chain(pypi_dependencies)
            .collect::<VecDeque<_>>();

        // Construct a queue of dependencies that we wanna find in the lock file
        let mut queue = dependencies.clone();

        // Get the virtual packages for the system
        let virtual_packages = project
            .virtual_packages(platform)
            .into_iter()
            .map(|vpkg| (vpkg.name.clone(), vpkg))
            .collect::<HashMap<_, _>>();

        // Keep track of which dependencies we already found. Since there can always only be one
        // version per named package we can just keep track of the package names.
        let mut seen = dependencies
            .iter()
            .filter_map(|req| match req {
                DependencyKind::Conda(spec) => spec.name.clone().map(DependencyName::Conda),
                DependencyKind::PyPi(req) => Some(DependencyName::PyPi(
                    NormalizedPackageName::from_str(&req.name).ok()?,
                )),
            })
            .collect::<HashSet<_>>();

        while let Some(dependency) = queue.pop_back() {
            let locked_package = match &dependency {
                DependencyKind::Conda(match_spec) => {
                    // Is this a virtual package? And does it match?
                    if let Some(vpkg) = match_spec
                        .name
                        .as_ref()
                        .and_then(|name| virtual_packages.get(name))
                    {
                        if let Some(version_spec) = &match_spec.version {
                            if !version_spec.matches(&vpkg.version) {
                                tracing::info!("found a dependency on virtual package '{}' but the version spec '{}' does not match the expected version of the virtual package '{}'.", vpkg.name.as_source(), &version_spec, &vpkg.version);
                                return Ok(false);
                            }
                        }
                        if let Some(build_spec) = &match_spec.build {
                            if !build_spec.matches(&vpkg.build_string) {
                                tracing::info!("found a dependency on virtual package '{}' but the build spec '{}' does not match the expected build of the virtual package '{}'.", vpkg.name.as_source(), &build_spec, &vpkg.build_string);
                                return Ok(false);
                            }
                        }

                        // Virtual package matches
                        continue;
                    }

                    // Find the package in the lock-file that matches our dependency.
                    lock_file
                        .get_packages_by_platform(platform)
                        .find(|locked_package| {
                            locked_dependency_satisfies_match_spec(locked_package, match_spec)
                        })
                }
                DependencyKind::PyPi(requirement) => {
                    // Find the package in the lock-file that matches our requirement.
                    lock_file
                        .get_packages_by_platform(platform)
                        .find_map(|locked_package| {
                            match locked_dependency_satisfies_requirement(locked_package, requirement) {
                                Ok(true) => Some(Ok(locked_package)),
                                Ok(false) => None,
                                Err(e) => {
                                    tracing::info!("failed to check if locked package '{}' satisfies requirement '{}': {e}", locked_package.name, requirement);
                                    Some(Err(e))
                                }
                            }
                        }).transpose()?
                }
            };

            match locked_package {
                None => {
                    // No package found that matches the dependency, the lock file is not in a
                    // consistent state.
                    tracing::info!("failed to find a locked package for '{}', assuming the lock file is out of date.", &dependency);
                    return Ok(false);
                }
                Some(package) => match &package.kind {
                    LockedDependencyKind::Conda(conda_package) => {
                        for spec in conda_package.dependencies.iter() {
                            let Ok(spec) = MatchSpec::from_str(spec) else {
                                tracing::warn!(
                                    "failed to parse spec '{}', assuming the lock file is corrupt.",
                                    spec
                                );
                                return Ok(false);
                            };

                            if let Some(name) = spec.name.clone() {
                                let dependency_name = DependencyName::Conda(name);
                                if !seen.contains(&dependency_name) {
                                    queue.push_back(DependencyKind::Conda(spec));
                                    seen.insert(dependency_name);
                                }
                            }
                        }
                    }
                    LockedDependencyKind::Pypi(pypi_package) => {
                        // TODO: We have to verify that the python version is compatible

                        for req in pypi_package.requires_dist.iter() {
                            let Ok(req) = pep508_rs::Requirement::from_str(req) else {
                                tracing::warn!(
                                    "failed to parse requirement '{}', assuming the lock file is corrupt.",
                                    req
                                );
                                return Ok(false);
                            };
                            // Filter the requirement based on the environment markers
                            if !python_marker_env
                                .as_ref()
                                .map(|env| {
                                    req.evaluate_markers(
                                        env,
                                        pypi_package.extras.iter().cloned().collect_vec(),
                                    )
                                })
                                .unwrap_or(true)
                            {
                                continue;
                            }
                            let Ok(name) = rip::types::NormalizedPackageName::from_str(&req.name)
                            else {
                                tracing::warn!(
                                    "failed to parse package name '{}', assuming the lock file is corrupt.",
                                    req.name
                                );
                                return Ok(false);
                            };
                            let dependency_name = DependencyName::PyPi(name);
                            if !seen.contains(&dependency_name) {
                                queue.push_back(DependencyKind::PyPi(req));
                                seen.insert(dependency_name);
                            }
                        }
                    }
                },
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

/// Check whether the specified requirement is satisfied by the given locked package.
fn locked_dependency_satisfies_requirement(
    locked_package: &LockedDependency,
    requirement: &Requirement,
) -> miette::Result<bool> {
    let pypi_packages =
        package_identifier::PypiPackageIdentifier::from_locked_dependency(locked_package)
            .into_diagnostic()?;
    Ok(pypi_packages
        .into_iter()
        .any(|pypi_package| pypi_package.satisfies(requirement)))
}

/// Returns true if the specified [`conda_lock::LockedDependency`] satisfies the given MatchSpec.
/// TODO: Move this back to rattler.
/// TODO: Make this more elaborate to include all properties of MatchSpec
fn locked_dependency_satisfies_match_spec(
    locked_package: &LockedDependency,
    match_spec: &MatchSpec,
) -> bool {
    // Only conda packages can match matchspecs.
    let Some(conda) = locked_package.as_conda() else {
        return false;
    };

    // Check if the name of the package matches
    if match_spec
        .name
        .as_ref()
        .map(|name| locked_package.name != name.as_normalized())
        .unwrap_or(false)
    {
        return false;
    }

    // Check if the version matches
    if let Some(version_spec) = &match_spec.version {
        let v = match Version::from_str(&locked_package.version) {
            Err(_) => return false,
            Ok(v) => v,
        };

        if !version_spec.matches(&v) {
            return false;
        }
    }

    // Check if the build string matches
    match (match_spec.build.as_ref(), &conda.build) {
        (Some(build_spec), Some(build)) => {
            if !build_spec.matches(build) {
                return false;
            }
        }
        (Some(_), None) => return false,
        _ => {}
    }

    // If there is a channel specified, check if the channel matches
    if let Some(channel) = &match_spec.channel {
        if !conda.url.as_str().starts_with(channel.base_url.as_str()) {
            return false;
        }
    }

    true
}
