use super::{PypiRecord, PypiRecordsByName, RepoDataRecordsByName};
use crate::{project::Environment, pypi_marker_env::determine_marker_environment};
use itertools::Itertools;
use miette::Diagnostic;
use pep440_rs::VersionSpecifiers;
use pep508_rs::Requirement;
use rattler_conda_types::ParseStrictness::Lenient;
use rattler_conda_types::{
    GenericVirtualPackage, MatchSpec, ParseMatchSpecError, Platform, RepoDataRecord,
};
use rattler_lock::{ConversionError, Package};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
};
use thiserror::Error;
use uv_normalize::{ExtraName, PackageName};

#[derive(Debug, Error, Diagnostic)]
pub enum EnvironmentUnsat {
    #[error("the channels in the lock-file do not match the environments channels")]
    ChannelsMismatch,
}

#[derive(Debug, Error, Diagnostic)]
pub enum PlatformUnsat {
    #[error("the requirement '{0}' could not be satisfied (required by '{1}')")]
    UnsatisfiableMatchSpec(MatchSpec, String),

    #[error("the requirement '{0}' could not be satisfied (required by '{1}')")]
    UnsatisfiableRequirement(Requirement, String),

    #[error("there was a duplicate entry for '{0}'")]
    DuplicateEntry(String),

    #[error("the requirement '{0}' failed to parse")]
    FailedToParseMatchSpec(String, #[source] ParseMatchSpecError),

    #[error("there are more conda packages in the lock-file than are used by the environment")]
    TooManyCondaPackages,

    #[error("corrupted lock-file entry for '{0}'")]
    CorruptedEntry(String, ConversionError),

    #[error("there are more pypi packages in the lock-file than are used by the environment: {}", .0.iter().format(", "))]
    TooManyPypiPackages(Vec<PackageName>),

    #[error("there are PyPi dependencies but a python interpreter is missing from the lock-file")]
    MissingPythonInterpreter,

    #[error(
        "a marker environment could not be derived from the python interpreter in the lock-file"
    )]
    FailedToDetermineMarkerEnvironment(#[source] Box<dyn Diagnostic + Send + Sync>),

    #[error("{0} requires python version {1} but the python interpreter in the lock-file has version {2}")]
    PythonVersionMismatch(PackageName, VersionSpecifiers, Box<pep440_rs::Version>),
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
    // Convert the lock file into a list of conda and pypi packages
    let mut conda_packages: Vec<RepoDataRecord> = Vec::new();
    let mut pypi_packages: Vec<PypiRecord> = Vec::new();
    for package in locked_environment.packages(platform).into_iter().flatten() {
        match package {
            Package::Conda(conda) => {
                let url = conda.url().clone();
                conda_packages.push(
                    conda
                        .try_into()
                        .map_err(|e| PlatformUnsat::CorruptedEntry(url.to_string(), e))?,
                );
            }
            Package::Pypi(pypi) => {
                pypi_packages.push((pypi.data().package.clone(), pypi.data().environment.clone()));
            }
        }
    }

    // Create a lookup table from package name to package record. Returns an error if we find a
    // duplicate entry for a record
    let repodata_records_by_name = match RepoDataRecordsByName::from_unique_iter(conda_packages) {
        Ok(conda_packages) => conda_packages,
        Err(duplicate) => {
            return Err(PlatformUnsat::DuplicateEntry(
                duplicate.package_record.name.as_source().to_string(),
            ))
        }
    };

    // Create a lookup table from package name to package record. Returns an error if we find a
    // duplicate entry for a record
    let pypi_records_by_name = match PypiRecordsByName::from_unique_iter(pypi_packages) {
        Ok(conda_packages) => conda_packages,
        Err(duplicate) => return Err(PlatformUnsat::DuplicateEntry(duplicate.0.name.to_string())),
    };

    verify_package_platform_satisfiability(
        environment,
        &repodata_records_by_name,
        &pypi_records_by_name,
        platform,
    )
}

enum Dependency {
    Conda(MatchSpec, Cow<'static, str>),
    PyPi(Requirement, Cow<'static, str>),
}

pub fn verify_package_platform_satisfiability(
    environment: &Environment<'_>,
    locked_conda_packages: &RepoDataRecordsByName,
    locked_pypi_environment: &PypiRecordsByName,
    platform: Platform,
) -> Result<(), PlatformUnsat> {
    // Determine the dependencies requested by the environment
    let conda_specs = environment
        .dependencies(None, Some(platform))
        .into_match_specs()
        .map(|spec| Dependency::Conda(spec, "<environment>".into()))
        .collect_vec();

    if conda_specs.is_empty() && !locked_conda_packages.is_empty() {
        return Err(PlatformUnsat::TooManyCondaPackages);
    }

    let pypi_requirements = environment
        .pypi_dependencies(Some(platform))
        .iter()
        .flat_map(|(name, reqs)| {
            reqs.iter().map(move |req| {
                Dependency::PyPi(req.as_pep508(name.as_normalized()), "<environment>".into())
            })
        })
        .collect_vec();

    if pypi_requirements.is_empty() && !locked_pypi_environment.is_empty() {
        return Err(PlatformUnsat::TooManyPypiPackages(
            locked_pypi_environment.names().cloned().collect(),
        ));
    }

    // Create a list of virtual packages by name
    let virtual_packages = environment
        .virtual_packages(platform)
        .into_iter()
        .map(|vpkg| (vpkg.name.clone(), vpkg))
        .collect::<HashMap<_, _>>();

    // Find the python interpreter from the list of conda packages. Note that this refers to the
    // locked python interpreter, it might not match the specs from the environment. That is ok
    // because we will find that out when we check all the records.
    let python_interpreter_record = locked_conda_packages.python_interpreter_record();

    // Determine the marker environment from the python interpreter package.
    let marker_environment = python_interpreter_record
        .map(|interpreter| determine_marker_environment(platform, &interpreter.package_record))
        .transpose()
        .map_err(|err| PlatformUnsat::FailedToDetermineMarkerEnvironment(err.into()))?;

    // Determine the pypi packages provided by the locked conda packages.
    let locked_conda_pypi_packages = locked_conda_packages.by_pypi_name();

    // Keep a list of all conda packages that we have already visisted
    let mut conda_packages_visited = HashSet::new();
    let mut pypi_packages_visited = HashSet::new();
    let mut pypi_requirements_visited = pypi_requirements
        .iter()
        .filter_map(|r| match r {
            Dependency::PyPi(req, _) => Some(req.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();

    // Iterate over all packages. First iterate over all conda matchspecs and then over all pypi
    // requirements. We want to ensure we always check the conda packages first.
    let mut conda_queue = conda_specs;
    let mut pypi_queue = pypi_requirements;
    while let Some(package) = conda_queue.pop().or_else(|| pypi_queue.pop()) {
        enum FoundPackage {
            Conda(usize),
            PyPi(usize, Vec<ExtraName>),
        }

        // Determine the package that matches the requirement of matchspec.
        let found_package = match package {
            Dependency::Conda(spec, source) => {
                match &spec.name {
                    None => {
                        // No name means we have to find any package that matches the spec.
                        match locked_conda_packages
                            .records
                            .iter()
                            .position(|record| record.matches(&spec))
                        {
                            None => {
                                // No records match the spec.
                                return Err(PlatformUnsat::UnsatisfiableMatchSpec(
                                    spec,
                                    source.into_owned(),
                                ));
                            }
                            Some(idx) => FoundPackage::Conda(idx),
                        }
                    }
                    Some(name) => {
                        match locked_conda_packages
                            .index_by_name(name)
                            .map(|idx| (idx, &locked_conda_packages.records[idx]))
                        {
                            Some((idx, record)) if record.matches(&spec) => {
                                FoundPackage::Conda(idx)
                            }
                            Some(_) => {
                                // The record does not match the spec, the lock-file is inconsistent.
                                return Err(PlatformUnsat::UnsatisfiableMatchSpec(
                                    spec,
                                    source.into_owned(),
                                ));
                            }
                            None => {
                                // Check if there is a virtual package by that name
                                if let Some(vpkg) = virtual_packages.get(name.as_normalized()) {
                                    if vpkg.matches(&spec) {
                                        // The matchspec matches a virtual package. No need to propagate the dependencies.
                                        continue;
                                    } else {
                                        // The record does not match the spec, the lock-file is inconsistent.
                                        return Err(PlatformUnsat::UnsatisfiableMatchSpec(
                                            spec,
                                            source.into_owned(),
                                        ));
                                    }
                                } else {
                                    // The record does not match the spec, the lock-file is inconsistent.
                                    return Err(PlatformUnsat::UnsatisfiableMatchSpec(
                                        spec,
                                        source.into_owned(),
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            Dependency::PyPi(requirement, source) => {
                // Check if there is a pypi identifier that matches our requirement.
                if let Some((identifier, repodata_idx, _)) =
                    locked_conda_pypi_packages.get(&requirement.name)
                {
                    if identifier.satisfies(&requirement) {
                        FoundPackage::Conda(*repodata_idx)
                    } else {
                        // The record does not match the spec, the lock-file is inconsistent.
                        return Err(PlatformUnsat::UnsatisfiableRequirement(
                            requirement,
                            source.into_owned(),
                        ));
                    }
                } else if let Some(idx) = locked_pypi_environment.index_by_name(&requirement.name) {
                    let record = &locked_pypi_environment.records[idx];
                    if record.0.satisfies(&requirement) {
                        FoundPackage::PyPi(idx, requirement.extras)
                    } else {
                        // The record does not match the spec, the lock-file is inconsistent.
                        return Err(PlatformUnsat::UnsatisfiableRequirement(
                            requirement,
                            source.into_owned(),
                        ));
                    }
                } else {
                    // The record does not match the spec, the lock-file is inconsistent.
                    return Err(PlatformUnsat::UnsatisfiableRequirement(
                        requirement,
                        source.into_owned(),
                    ));
                }
            }
        };

        // Add all the requirements of the package to the queue.
        match found_package {
            FoundPackage::Conda(idx) => {
                if !conda_packages_visited.insert(idx) {
                    // We already visited this package, so we can skip adding its dependencies to the queue
                    continue;
                }

                let record = &locked_conda_packages.records[idx];
                for depends in &record.package_record.depends {
                    let spec = MatchSpec::from_str(depends.as_str(), Lenient)
                        .map_err(|e| PlatformUnsat::FailedToParseMatchSpec(depends.clone(), e))?;
                    conda_queue.push(Dependency::Conda(
                        spec,
                        Cow::Owned(record.file_name.clone()),
                    ));
                }
            }
            FoundPackage::PyPi(idx, extras) => {
                let record = &locked_pypi_environment.records[idx];

                // If there is no marker environment there is no python version
                let Some(marker_environment) = marker_environment.as_ref() else {
                    return Err(PlatformUnsat::MissingPythonInterpreter);
                };

                if pypi_packages_visited.insert(idx) {
                    // Ensure that the record matches the currently selected interpreter.
                    if let Some(python_version) = &record.0.requires_python {
                        if !python_version.contains(&marker_environment.python_full_version.version)
                        {
                            return Err(PlatformUnsat::PythonVersionMismatch(
                                record.0.name.clone(),
                                python_version.clone(),
                                marker_environment
                                    .python_full_version
                                    .version
                                    .clone()
                                    .into(),
                            ));
                        }
                    }
                }

                // Add all the requirements of the package to the queue.
                for requirement in &record.0.requires_dist {
                    if !pypi_requirements_visited.insert(requirement.clone()) {
                        continue;
                    }

                    // Skip this requirement if it does not apply.
                    if !requirement.evaluate_markers(marker_environment, &extras) {
                        continue;
                    }

                    pypi_queue.push(Dependency::PyPi(
                        requirement.clone(),
                        record.0.name.as_ref().to_string().into(),
                    ));
                }
            }
        }
    }

    // Check if all locked packages have also been visisted
    if conda_packages_visited.len() != locked_conda_packages.len() {
        return Err(PlatformUnsat::TooManyCondaPackages);
    }

    if pypi_packages_visited.len() != locked_pypi_environment.len() {
        return Err(PlatformUnsat::TooManyPypiPackages(
            locked_pypi_environment
                .names()
                .enumerate()
                .filter_map(|(idx, name)| {
                    if pypi_packages_visited.contains(&idx) {
                        None
                    } else {
                        Some(name.clone())
                    }
                })
                .collect(),
        ));
    }

    Ok(())
}

trait MatchesMatchspec {
    fn matches(&self, spec: &MatchSpec) -> bool;
}

impl MatchesMatchspec for RepoDataRecord {
    fn matches(&self, spec: &MatchSpec) -> bool {
        if !spec.matches(&self.package_record) {
            return false;
        }

        // TODO: We should really move this into rattler
        // Check the channel
        if let Some(channel) = &spec.channel {
            if !self.url.as_str().starts_with(channel.base_url.as_str()) {
                return false;
            }
        }

        true
    }
}

impl MatchesMatchspec for GenericVirtualPackage {
    fn matches(&self, spec: &MatchSpec) -> bool {
        if let Some(name) = &spec.name {
            if name != &self.name {
                return false;
            }
        }

        if let Some(version) = &spec.version {
            if !version.matches(&self.version) {
                return false;
            }
        }

        if let Some(build) = &spec.build {
            if !build.matches(&self.build_string) {
                return false;
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Project;
    use miette::{IntoDiagnostic, NarratableReportHandler};
    use rattler_lock::LockFile;
    use rstest::rstest;
    use std::path::PathBuf;

    #[derive(Error, Debug, Diagnostic)]
    enum LockfileUnsat {
        #[error("environment '{0}' is missing")]
        EnvironmentMissing(String),

        #[error("environment '{0}' does not satisfy the requirements of the project")]
        Environment(String, #[source] EnvironmentUnsat),

        #[error(
            "environment '{0}' does not satisfy the requirements of the project for platform '{1}"
        )]
        PlatformUnsat(String, Platform, #[source] PlatformUnsat),
    }

    fn verify_lockfile_satisfiability(
        project: &Project,
        lock_file: &LockFile,
    ) -> Result<(), LockfileUnsat> {
        for env in project.environments() {
            let locked_env = lock_file
                .environment(env.name().as_str())
                .ok_or_else(|| LockfileUnsat::EnvironmentMissing(env.name().to_string()))?;
            verify_environment_satisfiability(&env, &locked_env)
                .map_err(|e| LockfileUnsat::Environment(env.name().to_string(), e))?;

            for platform in env.platforms() {
                verify_platform_satisfiability(&env, &locked_env, platform).map_err(|e| {
                    LockfileUnsat::PlatformUnsat(env.name().to_string(), platform, e)
                })?;
            }
        }
        Ok(())
    }

    #[rstest]
    fn test_good_satisfiability(
        #[files("tests/satisfiability/*/pixi.toml")] manifest_path: PathBuf,
    ) {
        let project = Project::load(&manifest_path).unwrap();
        let lock_file = LockFile::from_path(&project.lock_file_path()).unwrap();
        match verify_lockfile_satisfiability(&project, &lock_file).into_diagnostic() {
            Ok(()) => {}
            Err(e) => panic!("{e:?}"),
        }
    }

    #[rstest]
    fn test_example_satisfiability(#[files("examples/*/pixi.toml")] manifest_path: PathBuf) {
        let project = Project::load(&manifest_path).unwrap();
        let lock_file = LockFile::from_path(&project.lock_file_path()).unwrap();
        match verify_lockfile_satisfiability(&project, &lock_file).into_diagnostic() {
            Ok(()) => {}
            Err(e) => panic!("{e:?}"),
        }
    }

    #[test]
    fn test_failing_satisiability() {
        let report_handler = NarratableReportHandler::new().with_cause_chain();

        insta::glob!("../../tests/non-satisfiability", "*/pixi.toml", |path| {
            let project = Project::load(&path).unwrap();
            let lock_file = LockFile::from_path(&project.lock_file_path()).unwrap();
            let err = verify_lockfile_satisfiability(&project, &lock_file)
                .expect_err("expected failing satisfiability");

            let mut s = String::new();
            report_handler.render_report(&mut s, &err).unwrap();
            insta::assert_snapshot!(s);
        });
    }
}
