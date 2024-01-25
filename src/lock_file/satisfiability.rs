use super::package_identifier;
use crate::{
    project::Environment, pypi_marker_env::determine_marker_environment,
    pypi_tags::is_python_record, Project,
};
use itertools::Itertools;
use miette::Diagnostic;
use pep440_rs::VersionSpecifiers;
use pep508_rs::Requirement;
use rattler_conda_types::{MatchSpec, ParseMatchSpecError, Platform};
use rattler_lock::{CondaPackage, LockFile, Package, PypiPackage};
use rip::types::NormalizedPackageName;
use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
};
use thiserror::Error;

#[derive(Debug, Error, Diagnostic)]
pub enum Unsat {
    #[error("the environment '{0}' is unsatisfiable")]
    EnvironmentUnsatisfiable(String, #[source] EnvironmentUnsat),
}

#[derive(Debug, Error, Diagnostic)]
pub enum EnvironmentUnsat {
    #[error("the environment is not present in the lock-file")]
    Missing,

    #[error("channels mismatch")]
    ChannelsMismatch,
}

#[derive(Debug, Error, Diagnostic)]
pub enum PlatformUnsat {
    #[error("could not satisfy '{0}' (required by '{1}')")]
    UnsatisfiableMatchSpec(MatchSpec, String),

    #[error("could not satisfy '{0}' (required by '{1}')")]
    UnsatisfiableRequirement(Requirement, String),

    #[error("found a duplicate entry for '{0}'")]
    DuplicateEntry(String),

    #[error("failed to parse requirement '{0}'")]
    FailedToParseMatchSpec(String, #[source] ParseMatchSpecError),

    #[error("too many conda packages in the lock-file")]
    TooManyCondaPackages,

    #[error("too many pypi packages in the lock-file")]
    TooManyPypiPackages,

    #[error("there are PyPi dependencies but a python interpreter is missing from the lock-file")]
    MissingPythonInterpreter,

    #[error("failed to determine marker environment from the python interpreter in the lock-file")]
    FailedToDetermineMarkerEnvironment(#[source] Box<dyn Diagnostic + Send + Sync>),

    #[error("{0} requires python version {1} but the python interpreter in the lock-file has version {2}")]
    PythonVersionMismatch(String, VersionSpecifiers, Box<pep440_rs::Version>),
}

/// A helper method to check if the lock file satisfies the project.
///
/// This function checks all environments and all platforms of each environment. The function early
/// outs if verification of any environment fails.
pub fn lock_file_satisfies_project(project: &Project, lock_file: &LockFile) -> Result<(), Unsat> {
    for env in project.environments() {
        let Some(locked_env) = lock_file.environment(env.name().as_str()) else {
            return Err(Unsat::EnvironmentUnsatisfiable(
                env.name().as_str().to_string(),
                EnvironmentUnsat::Missing,
            ));
        };

        verify_environment_satisfiability(&env, &locked_env).map_err(|unsat| {
            Unsat::EnvironmentUnsatisfiable(env.name().as_str().to_string(), unsat)
        })?
    }

    Ok(())
}

/// Verifies that all the requirements of the specified `environment` can be satisfied with the
/// packages present in the lock-file.
///
/// This function returns a [`EnvironmentUnsat`] error if a verification issue occurred. The
/// [`EnvironmentUnsat`] error should contain enough information for the user and developer to
/// figure out what went wrong.
pub fn verify_environment_satisfiability(
    environment: &Environment<'_>,
    locked_environment: &rattler_lock::Environment,
) -> Result<(), EnvironmentUnsat> {
    // Check if the channels in the lock file match our current configuration. Note that the order
    // matters here. If channels are added in a different order, the solver might return a different
    // result.
    let channels = environment
        .channels()
        .into_iter()
        .map(|channel| rattler_lock::Channel::from(channel.base_url().to_string()))
        .collect_vec();
    if !locked_environment.channels().eq(&channels) {
        return Err(EnvironmentUnsat::ChannelsMismatch);
    }

    Ok(())
}

/// Verifies that the package requirements of the specified `environment` can be satisfied with the
/// packages present in the lock-file.
///
/// Both Conda and pypi packages are verified by this function. First all the conda package are
/// verified and then all the pypi packages are verified. This is done so that if we can check if
/// we only need to update the pypi dependencies or also the conda dependencies.
///
/// This function returns a [`PlatformUnsat`] error if a verification issue occurred. The
/// [`PlatformUnsat`] error should contain enough information for the user and developer to figure
/// out what went wrong.
pub fn verify_platform_satisfiability(
    environment: &Environment<'_>,
    locked_environment: &rattler_lock::Environment,
    platform: Platform,
) -> Result<(), PlatformUnsat> {
    // Get all the conda packages from the locked environment
    let conda_packages = locked_environment
        .packages(platform)
        .into_iter()
        .flatten()
        .filter_map(Package::into_conda)
        .collect_vec();

    // Check the satisfiability of the conda packages.
    verify_conda_platform_satisfiability(environment, &conda_packages, platform)?;

    // Get all the pypi packages from the locked environment
    let pypi_packages = locked_environment
        .packages(platform)
        .into_iter()
        .flatten()
        .filter_map(Package::into_pypi)
        .collect_vec();

    // Check the satisfiability of the pypi packages.
    verify_pypi_platform_satisfiability(environment, &conda_packages, &pypi_packages, platform)?;

    Ok(())
}

pub fn verify_conda_platform_satisfiability(
    environment: &Environment<'_>,
    locked_environment: &Vec<CondaPackage>,
    platform: Platform,
) -> Result<(), PlatformUnsat> {
    // Get all the requirements from the environment
    let mut specs = environment
        .dependencies(None, Some(platform))
        .into_match_specs()
        .map(|spec| (spec, "<environment>"))
        .collect_vec();

    // Create a lookup table from package name to package record. Returns an error if we find a
    // duplicate entry for a record.
    let mut name_to_record = HashMap::new();
    for (record_idx, record) in locked_environment.iter().enumerate() {
        if name_to_record
            .insert(
                record.package_record().name.as_normalized().to_string(),
                record_idx,
            )
            .is_some()
        {
            return Err(PlatformUnsat::DuplicateEntry(
                record.package_record().name.as_normalized().to_string(),
            ));
        }
    }

    // Create a list of virtual packages by name
    let virtual_packages = environment
        .virtual_packages(platform)
        .into_iter()
        .map(|vpkg| (vpkg.name.clone(), vpkg))
        .collect::<HashMap<_, _>>();

    // Keep a list of all records we have seen.
    let mut records_visited = HashSet::new();

    // Iterate over all the requirements and find all packages that match the requirements. If
    while let Some((spec, source)) = specs.pop() {
        let matching_record_idx = match &spec.name {
            None => {
                // If the spec does not define a name this means we have to find any record that
                // matches the spec.
                locked_environment.iter().position(|r| r.satisfies(&spec))
            }
            Some(name) => {
                // If the spec does define a name we can do a quick lookup based on the name.
                //
                // We start by looking at virtual packages. This is also what the solver does. It
                // first tries to find a virtual package that matches the spec followed by regular
                // packages.
                if let Some(vpkg) = virtual_packages.get(name) {
                    if spec
                        .version
                        .as_ref()
                        .map(|spec| spec.matches(&vpkg.version))
                        .unwrap_or(true)
                        && spec
                            .build
                            .as_ref()
                            .map(|spec| spec.matches(&vpkg.build_string))
                            .unwrap_or(true)
                    {
                        // Virtual package matches
                        continue;
                    }
                }

                // Otherwise, find the record that matches the spec.
                name_to_record
                    .get(name.as_normalized())
                    .copied()
                    .and_then(|idx| {
                        let record = &locked_environment[idx];
                        if record.satisfies(&spec) {
                            Some(idx)
                        } else {
                            None
                        }
                    })
            }
        };

        // Bail if no record could be found
        let Some(matching_record_idx) = matching_record_idx else {
            return Err(PlatformUnsat::UnsatisfiableMatchSpec(
                spec,
                source.to_string(),
            ));
        };

        // Check if we've already seen this package
        if !records_visited.insert(matching_record_idx) {
            continue;
        }

        // Otherwise, add all the requirements of the record to the queue.
        let record = &locked_environment[matching_record_idx];
        let source = record
            .file_name()
            .unwrap_or_else(|| record.package_record().name.as_normalized());
        for depends in &record.package_record().depends {
            let spec = MatchSpec::from_str(depends.as_str())
                .map_err(|e| PlatformUnsat::FailedToParseMatchSpec(depends.clone(), e))?;
            specs.push((spec, source))
        }
    }

    // If we didn't visit all conda-records it means we have too many packages in the lock-file. We
    // don't want to install more packages than we need.
    if records_visited.len() != locked_environment.len() {
        return Err(PlatformUnsat::TooManyCondaPackages);
    }

    Ok(())
}

pub fn verify_pypi_platform_satisfiability(
    environment: &Environment<'_>,
    locked_conda_packages: &[CondaPackage],
    locked_pypi_environment: &[PypiPackage],
    platform: Platform,
) -> Result<(), PlatformUnsat> {
    let mut requirements = environment
        .pypi_dependencies(Some(platform))
        .iter()
        .flat_map(|(name, reqs)| {
            reqs.iter()
                .map(move |req| (req.as_pep508(name), "<environment>"))
        })
        .collect_vec();

    // If there are no pypi packages specified in the requirement, we can skip verifying them.
    if requirements.is_empty() {
        return if !locked_pypi_environment.is_empty() {
            Err(PlatformUnsat::TooManyPypiPackages)
        } else {
            Ok(())
        };
    }

    // Construct package identifiers for the locked packages
    let package_identifiers =
        package_identifiers_from_locked_packages(locked_conda_packages, locked_pypi_environment);

    // Create a lookup by name for the package identifiers.
    let mut name_to_package_identifiers = HashMap::new();
    for (idx, (identifier, _)) in package_identifiers.iter().enumerate() {
        name_to_package_identifiers
            .entry(identifier.name.clone())
            .or_insert_with(Vec::new)
            .push(idx);
    }

    // Find the python package from the list of conda packages.
    let Some(python_record) = locked_conda_packages.iter().find(|r| is_python_record(*r)) else {
        return Err(PlatformUnsat::MissingPythonInterpreter);
    };

    // Determine the marker environment from the python interpreter package
    let marker_environment =
        match determine_marker_environment(platform, python_record.package_record()) {
            Ok(marker_environment) => marker_environment,
            Err(e) => return Err(PlatformUnsat::FailedToDetermineMarkerEnvironment(e.into())),
        };

    // Keep a list of all requirements we have seen so we don't check them again.
    let mut requirements_visited = requirements
        .iter()
        .map(|(req, _source)| req.clone())
        .collect::<HashSet<_>>();

    // Keep a list of all packages visited
    let mut packages_visited = HashSet::new();

    // Iterate over all the requirements and find a packages that match the requirements.
    while let Some((requirement, source)) = requirements.pop() {
        // Convert the name to a normalized string. If the name is not valid, we also won't be able
        // to satisfy the requirement.
        let Ok(name) = NormalizedPackageName::from_str(requirement.name.as_str()) else {
            return Err(PlatformUnsat::UnsatisfiableRequirement(
                requirement,
                source.to_string(),
            ));
        };

        // Look-up the identifier that matches the requirement
        let matched_package = name_to_package_identifiers
            .get(&name)
            .into_iter()
            .flat_map(|idxs| idxs.iter().map(|idx| &package_identifiers[*idx]))
            .find(|(identifier, _pypi_package_idx)| identifier.satisfies(&requirement));

        // Error if no package could be found that matches the requirement
        let Some((_identifier, pypi_package_idx)) = matched_package else {
            return Err(PlatformUnsat::UnsatisfiableRequirement(
                requirement,
                source.to_string(),
            ));
        };

        // Get the package data from the found package. Or if there is no package data, continue,
        // because that indicates that the package is a conda package.
        let Some(pypi_package_idx) = *pypi_package_idx else {
            continue;
        };
        let pkg_data = locked_pypi_environment[pypi_package_idx].data().package;

        // Record that we visited this package.
        packages_visited.insert(pypi_package_idx);

        // Check that the package is compatible with the python version
        if let Some(required_python_version) = &pkg_data.requires_python {
            if !required_python_version.contains(&marker_environment.python_full_version.version) {
                return Err(PlatformUnsat::PythonVersionMismatch(
                    pkg_data.name.clone(),
                    required_python_version.clone(),
                    marker_environment
                        .python_full_version
                        .version
                        .clone()
                        .into(),
                ));
            }
        }

        // Loop over all requirements of the package and add them to the queue.
        for dependency in pkg_data.requires_dist.iter() {
            // Skip this requirement if it does not apply.
            if !dependency.evaluate_markers(
                &marker_environment,
                requirement.extras.clone().unwrap_or_default(),
            ) {
                continue;
            }

            // Make sure we don't visit the same requirement twice.
            if requirements_visited.get(dependency).is_some() {
                continue;
            }

            // Add the requirement to the queue.
            requirements_visited.insert(dependency.clone());
            requirements.push((dependency.clone(), &pkg_data.name));
        }
    }

    // Make sure we don't have more packages than we need.
    if packages_visited.len() != locked_pypi_environment.len() {
        return Err(PlatformUnsat::TooManyPypiPackages);
    }

    Ok(())
}

/// Returns the [`PypiPackageIdentifier`] that are present in the given set of locked packages. The
/// resulting identifiers are also associated with the package data that they came from. This is
/// only the case for Pypi packages.
///
/// Both Conda and Pypi package have [`PypiPackageIdentifier`]s associated with them.
fn package_identifiers_from_locked_packages(
    locked_conda_environment: &[CondaPackage],
    locked_pypi_environment: &[PypiPackage],
) -> Vec<(package_identifier::PypiPackageIdentifier, Option<usize>)> {
    // Construct package identifiers for the conda locked packages
    let conda_package_identifiers = locked_conda_environment
        .iter()
        .map(package_identifier::PypiPackageIdentifier::from_locked_conda_dependency)
        .filter_map(Result::ok)
        .flatten()
        .map(|pkg| (pkg, None));

    // Construct package identifiers from the locked pypi packages. Also associate the resulting
    // identifiers with the package data that it came from.
    let pypi_package_identifiers =
        locked_pypi_environment
            .iter()
            .enumerate()
            .filter_map(|(idx, pypi_package)| {
                Some((
                    package_identifier::PypiPackageIdentifier::from_locked_pypi_dependency(
                        pypi_package,
                    )
                    .ok()?,
                    Some(idx),
                ))
            });

    // Combine the two sets of identifiers.
    itertools::chain(conda_package_identifiers, pypi_package_identifiers).collect()
}
