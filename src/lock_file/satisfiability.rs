use super::{PypiRecord, PypiRecordsByName, RepoDataRecordsByName};
use crate::project::manifest::python::{AsPep508Error, RequirementOrEditable};
use crate::{project::Environment, pypi_marker_env::determine_marker_environment};
use distribution_types::DirectGitUrl;
use itertools::Itertools;
use miette::Diagnostic;
use pep440_rs::VersionSpecifiers;
use pep508_rs::{Requirement, VersionOrUrl};
use rattler_conda_types::ParseStrictness::Lenient;
use rattler_conda_types::{
    GenericVirtualPackage, MatchSpec, ParseMatchSpecError, Platform, RepoDataRecord,
};
use rattler_lock::{ConversionError, Package, PypiPackageData, PypiSourceTreeHashable, UrlOrPath};
use requirements_txt::EditableRequirement;
use std::fmt::Display;
use std::ops::Sub;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
};
use thiserror::Error;
use url::Url;
use uv_git::GitReference;
use uv_normalize::{ExtraName, PackageName};

#[derive(Debug, Error, Diagnostic)]
pub enum EnvironmentUnsat {
    #[error("the channels in the lock-file do not match the environments channels")]
    ChannelsMismatch,
}

#[derive(Debug, Error)]
pub struct EditablePackagesMismatch {
    pub expected_editable: Vec<PackageName>,
    pub unexpected_editable: Vec<PackageName>,
}

#[derive(Debug, Error, Diagnostic)]
pub enum PlatformUnsat {
    #[error("the requirement '{0}' could not be satisfied (required by '{1}')")]
    UnsatisfiableMatchSpec(MatchSpec, String),

    #[error("the requirement '{0}' could not be satisfied (required by '{1}')")]
    UnsatisfiableRequirement(RequirementOrEditable, String),

    #[error("the conda package does not satisfy the pypi requirement '{0}' (required by '{1}')")]
    CondaUnsatisfiableRequirement(Requirement, String),

    #[error("there was a duplicate entry for '{0}'")]
    DuplicateEntry(String),

    #[error("the requirement '{0}' failed to parse")]
    FailedToParseMatchSpec(String, #[source] ParseMatchSpecError),

    #[error("there are more conda packages in the lock-file than are used by the environment")]
    TooManyCondaPackages,

    #[error("missing purls")]
    MissingPurls,

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

    #[error("when converting {0} into a pep508 requirement")]
    AsPep508Error(PackageName, #[source] AsPep508Error),

    #[error("editable pypi dependency on conda resolved package '{0}' is not supported")]
    EditableDependencyOnCondaInstalledPackage(PackageName, Box<EditableRequirement>),

    #[error("direct pypi url dependency to a conda installed package '{0}' is not supported")]
    DirectUrlDependencyOnCondaInstalledPackage(PackageName),

    #[error(transparent)]
    EditablePackageMismatch(EditablePackagesMismatch),

    #[error("failed to determine pypi source tree hash for {0}")]
    FailedToDetermineSourceTreeHash(PackageName, std::io::Error),

    #[error("source tree hash for {0} does not match the hash in the lock-file")]
    SourceTreeHashMismatch(PackageName),

    #[error("the path '{0}, cannot be canonicalized")]
    FailedToCanonicalizePath(PathBuf, #[source] std::io::Error),
}

impl PlatformUnsat {
    /// Returns true if this is a problem with pypi packages only. This means the conda packages
    /// are still considered valid.
    pub fn is_pypi_only(&self) -> bool {
        matches!(
            self,
            PlatformUnsat::UnsatisfiableRequirement(_, _)
                | PlatformUnsat::TooManyPypiPackages(_)
                | PlatformUnsat::AsPep508Error(_, _)
                | PlatformUnsat::FailedToDetermineSourceTreeHash(_, _)
                | PlatformUnsat::PythonVersionMismatch(_, _, _)
                | PlatformUnsat::EditablePackageMismatch(_)
                | PlatformUnsat::SourceTreeHashMismatch(_),
        )
    }
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
    project_root: &Path,
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

    // to reflect new purls for pypi packages
    // we need to invalidate the locked environment
    // if all conda packages have empty purls
    if environment.has_pypi_dependencies()
        && pypi_packages.is_empty()
        && !conda_packages
            .iter()
            .any(|record| !record.package_record.purls.is_empty())
    {
        {
            return Err(PlatformUnsat::MissingPurls);
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
        project_root,
    )
}

enum Dependency {
    Conda(MatchSpec, Cow<'static, str>),
    PyPi(RequirementOrEditable, Cow<'static, str>),
}

/// Check satatisfiability of a pypi requirement against a locked pypi package
/// This also does an additional check for git urls when using direct url references
pub fn pypi_satifisfies_editable(
    locked_data: &PypiPackageData,
    spec: &EditableRequirement,
) -> bool {
    let spec_url = &spec.url;

    // In the case that both the spec and the locked data are direct git urls
    // we need to compare the urls to see if they are the same
    let spec_git_url = DirectGitUrl::try_from(&spec_url.to_url()).ok();
    let locked_git_url = locked_data
        .url_or_path
        .as_url()
        .and_then(|url| DirectGitUrl::try_from(url).ok());

    // Both are git url's
    if let (Some(spec_git_url), Some(locked_data_url)) = (spec_git_url, locked_git_url) {
        let base_is_same = spec_git_url.url.repository() == locked_data_url.url.repository();

        // If the spec does not specify a revision than any will do
        // E.g `git.com/user/repo` is the same as `git.com/user/repo@adbdd`
        if *spec_git_url.url.reference() == GitReference::DefaultBranch {
            return base_is_same;
        }
        // If the spec does specify a revision than the revision must match
        base_is_same && spec_git_url.url.reference() == locked_data_url.url.reference()
    } else {
        let spec_path_or_url = spec_url
            .given()
            .and_then(|url| UrlOrPath::from_str(url).ok())
            .unwrap_or(UrlOrPath::Url(spec_url.to_url()));

        // Strip the direct+ prefix if it exists for the direct url
        // because this is not part of the `Requirement` spec
        // we use this to record that it is a direct url
        let locked_path_or_url = match locked_data.url_or_path.clone() {
            UrlOrPath::Url(url) => UrlOrPath::Url(
                url.as_ref()
                    .strip_prefix("direct+")
                    .and_then(|str| Url::parse(str).ok())
                    .unwrap_or(url),
            ),
            UrlOrPath::Path(path) => UrlOrPath::Path(path),
        };
        spec_path_or_url == locked_path_or_url
    }
}

/// Check satatisfiability of a pypi requirement against a locked pypi package
/// This also does an additional check for git urls when using direct url references
pub fn pypi_satifisfies_requirement(locked_data: &PypiPackageData, spec: &Requirement) -> bool {
    if spec.name != locked_data.name {
        return false;
    }

    // Check if the version of the requirement matches
    match &spec.version_or_url {
        None => true,
        Some(VersionOrUrl::VersionSpecifier(spec)) => spec.contains(&locked_data.version),
        Some(VersionOrUrl::Url(spec_url)) => {
            // In the case that both the spec and the locked data are direct git urls
            // we need to compare the urls to see if they are the same
            let spec_git_url = DirectGitUrl::try_from(&spec_url.to_url()).ok();
            let locked_git_url = locked_data
                .url_or_path
                .as_url()
                .and_then(|url| DirectGitUrl::try_from(url).ok());

            // Both are git url's
            if let (Some(spec_git_url), Some(locked_data_url)) = (spec_git_url, locked_git_url) {
                let base_is_same =
                    spec_git_url.url.repository() == locked_data_url.url.repository();

                // If the spec does not specify a revision than any will do
                // E.g `git.com/user/repo` is the same as `git.com/user/repo@adbdd`
                if *spec_git_url.url.reference() == GitReference::DefaultBranch {
                    return base_is_same;
                }
                // If the spec does specify a revision than the revision must match
                base_is_same && spec_git_url.url.reference() == locked_data_url.url.reference()
            } else {
                let spec_path_or_url = spec_url
                    .given()
                    .and_then(|url| UrlOrPath::from_str(url).ok())
                    .unwrap_or(UrlOrPath::Url(spec_url.to_url()));

                // Strip the direct+ prefix if it exists for the direct url
                // because this is not part of the `Requirement` spec
                // we use this to record that it is a direct url
                let locked_path_or_url = match locked_data.url_or_path.clone() {
                    UrlOrPath::Url(url) => UrlOrPath::Url(
                        url.as_ref()
                            .strip_prefix("direct+")
                            .and_then(|str| Url::parse(str).ok())
                            .unwrap_or(url),
                    ),
                    UrlOrPath::Path(path) => UrlOrPath::Path(path),
                };
                spec_path_or_url == locked_path_or_url
            }
        }
    }
}

pub fn verify_package_platform_satisfiability(
    environment: &Environment<'_>,
    locked_conda_packages: &RepoDataRecordsByName,
    locked_pypi_environment: &PypiRecordsByName,
    platform: Platform,
    project_root: &Path,
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
                Ok::<Dependency, PlatformUnsat>(Dependency::PyPi(
                    req.as_pep508(name.as_normalized(), project_root)
                        .map_err(|e| {
                            PlatformUnsat::AsPep508Error(name.as_normalized().clone(), e)
                        })?,
                    "<environment>".into(),
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    if pypi_requirements.is_empty() && !locked_pypi_environment.is_empty() {
        return Err(PlatformUnsat::TooManyPypiPackages(
            locked_pypi_environment.names().cloned().collect(),
        ));
    }

    // Create a list of virtual packages by name
    let virtual_packages = environment
        .virtual_packages(platform)
        .into_iter()
        .map(GenericVirtualPackage::from)
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
            Dependency::PyPi(RequirementOrEditable::Pep508Requirement(req), _) => Some(req.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();

    // Iterate over all packages. First iterate over all conda matchspecs and then over all pypi
    // requirements. We want to ensure we always check the conda packages first.
    let mut conda_queue = conda_specs;
    let mut pypi_queue = pypi_requirements;
    let mut expected_editable_pypi_packages = HashSet::new();
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
                    locked_conda_pypi_packages.get(requirement.name())
                {
                    // Check if the requirement is editable or a pep508 requirement
                    match requirement {
                        RequirementOrEditable::Editable(name, req) => {
                            return Err(PlatformUnsat::EditableDependencyOnCondaInstalledPackage(
                                name,
                                Box::new(req),
                            ));
                        }
                        RequirementOrEditable::Pep508Requirement(req)
                            if matches!(req.version_or_url, Some(VersionOrUrl::Url(_))) =>
                        {
                            return Err(PlatformUnsat::DirectUrlDependencyOnCondaInstalledPackage(
                                req.name.clone(),
                            ));
                        }
                        RequirementOrEditable::Pep508Requirement(req)
                            if !identifier.satisfies(&req) =>
                        {
                            // The record does not match the spec, the lock-file is inconsistent.
                            return Err(PlatformUnsat::CondaUnsatisfiableRequirement(
                                req,
                                source.into_owned(),
                            ));
                        }
                        _ => FoundPackage::Conda(*repodata_idx),
                    }
                } else if let Some(idx) = locked_pypi_environment.index_by_name(requirement.name())
                {
                    let record = &locked_pypi_environment.records[idx];
                    match requirement {
                        RequirementOrEditable::Editable(package_name, requirement) => {
                            if !pypi_satifisfies_editable(&record.0, &requirement) {
                                return Err(PlatformUnsat::UnsatisfiableRequirement(
                                    RequirementOrEditable::Editable(package_name, requirement),
                                    source.into_owned(),
                                ));
                            }

                            // Record that we want this package to be editable. This is used to
                            // check at the end if packages that should be editable are actually
                            // editable and vice versa.
                            expected_editable_pypi_packages.insert(package_name.clone());

                            FoundPackage::PyPi(idx, requirement.extras)
                        }
                        RequirementOrEditable::Pep508Requirement(requirement) => {
                            if !pypi_satifisfies_requirement(&record.0, &requirement) {
                                return Err(PlatformUnsat::UnsatisfiableRequirement(
                                    RequirementOrEditable::Pep508Requirement(requirement),
                                    source.into_owned(),
                                ));
                            }

                            FoundPackage::PyPi(idx, requirement.extras)
                        }
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
                    // If this is path based package we need to check if the source tree hash still matches.
                    // and if it is a directory
                    if let UrlOrPath::Path(path) = &record.0.url_or_path {
                        if path.is_dir() {
                            let path =
                                dunce::canonicalize(project_root.join(path)).map_err(|e| {
                                    PlatformUnsat::FailedToCanonicalizePath(path.clone(), e)
                                })?;
                            let hashable = PypiSourceTreeHashable::from_directory(path)
                                .map_err(|e| {
                                    PlatformUnsat::FailedToDetermineSourceTreeHash(
                                        record.0.name.clone(),
                                        e,
                                    )
                                })?
                                .hash();
                            if Some(hashable) != record.0.hash {
                                return Err(PlatformUnsat::SourceTreeHashMismatch(
                                    record.0.name.clone(),
                                ));
                            }
                        }
                    }

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
                    // Skip this requirement if it does not apply.
                    if !requirement.evaluate_markers(marker_environment, &extras) {
                        continue;
                    }

                    // Skip this requirement if it has already been visited.
                    if !pypi_requirements_visited.insert(requirement.clone()) {
                        continue;
                    }

                    pypi_queue.push(Dependency::PyPi(
                        RequirementOrEditable::Pep508Requirement(requirement.clone()),
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

    // Check if all packages that should be editable are actually editable and vice versa.
    let locked_editable_packages = locked_pypi_environment
        .records
        .iter()
        .filter(|record| record.0.editable)
        .map(|record| record.0.name.clone())
        .collect::<HashSet<_>>();
    let expected_editable = expected_editable_pypi_packages.sub(&locked_editable_packages);
    let unexpected_editable = locked_editable_packages.sub(&expected_editable_pypi_packages);
    if !expected_editable.is_empty() || !unexpected_editable.is_empty() {
        return Err(PlatformUnsat::EditablePackageMismatch(
            EditablePackagesMismatch {
                expected_editable: expected_editable.into_iter().sorted().collect(),
                unexpected_editable: unexpected_editable.into_iter().sorted().collect(),
            },
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

impl Display for EditablePackagesMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.expected_editable.is_empty() && self.unexpected_editable.is_empty() {
            write!(f, "expected ")?;
            format_package_list(f, &self.expected_editable)?;
            write!(
                f,
                " to be editable but in the lock-file {they} {are} not",
                they = it_they(self.expected_editable.len()),
                are = is_are(self.expected_editable.len())
            )?
        } else if self.expected_editable.is_empty() && !self.unexpected_editable.is_empty() {
            write!(f, "expected ")?;
            format_package_list(f, &self.unexpected_editable)?;
            write!(
                f,
                "NOT to be editable but in the lock-file {they} {are}",
                they = it_they(self.unexpected_editable.len()),
                are = is_are(self.unexpected_editable.len())
            )?
        } else {
            write!(f, "expected ")?;
            format_package_list(f, &self.expected_editable)?;
            write!(
                f,
                " to be editable but in the lock-file but {they} {are} not, whereas ",
                they = it_they(self.expected_editable.len()),
                are = is_are(self.expected_editable.len())
            )?;
            format_package_list(f, &self.unexpected_editable)?;
            write!(
                f,
                " {are} NOT expected to be editable which in the lock-file {they} {are}",
                they = it_they(self.unexpected_editable.len()),
                are = is_are(self.unexpected_editable.len())
            )?
        }

        return Ok(());

        fn format_package_list(
            f: &mut std::fmt::Formatter<'_>,
            packages: &[PackageName],
        ) -> std::fmt::Result {
            for (idx, package) in packages.iter().enumerate() {
                if idx == packages.len() - 1 && idx > 0 {
                    write!(f, " and ")?;
                } else if idx > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{}", package)?;
            }

            Ok(())
        }

        fn is_are(count: usize) -> &'static str {
            if count == 1 {
                "is"
            } else {
                "are"
            }
        }

        fn it_they(count: usize) -> &'static str {
            if count == 1 {
                "it"
            } else {
                "they"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Project;
    use miette::{IntoDiagnostic, NarratableReportHandler};
    use pep440_rs::Version;
    use rattler_lock::LockFile;
    use rstest::rstest;
    use std::ffi::OsStr;
    use std::{path::Component, path::PathBuf, str::FromStr};

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
                verify_platform_satisfiability(&env, &locked_env, platform, project.root())
                    .map_err(|e| {
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
        // TODO: skip this test on windows
        // Until we can figure out how to handle unix file paths with pep508_rs url parsing correctly
        if manifest_path
            .components()
            .contains(&Component::Normal(OsStr::new("absolute-paths")))
            && cfg!(windows)
        {
            return;
        }

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
            let project = Project::load(path).unwrap();
            let lock_file = LockFile::from_path(&project.lock_file_path()).unwrap();
            let err = verify_lockfile_satisfiability(&project, &lock_file)
                .expect_err("expected failing satisfiability");

            let mut s = String::new();
            report_handler.render_report(&mut s, &err).unwrap();
            insta::assert_snapshot!(s);
        });
    }

    #[test]
    fn test_pypi_git_check_with_rev() {
        // Mock locked datga
        let locked_data = PypiPackageData {
            name: "mypkg".parse().unwrap(),
            version: Version::from_str("0.1.0").unwrap(),
            url_or_path: "git+https://github.com/mypkg@abcd"
                .parse()
                .expect("failed to parse url"),
            hash: None,
            requires_dist: vec![],
            requires_python: None,
            editable: false,
        };
        let spec = Requirement::from_str("mypkg @ git+https://github.com/mypkg@abcd").unwrap();
        // This should satisfy:
        assert!(pypi_satifisfies_requirement(&locked_data, &spec));
        let non_matching_spec =
            Requirement::from_str("mypkg @ git+https://github.com/mypkg@defgd").unwrap();
        // This should not
        assert!(!pypi_satifisfies_requirement(
            &locked_data,
            &non_matching_spec
        ));
        // Removing the rev from the Requirement should satisfy any revision
        let spec = Requirement::from_str("mypkg @ git+https://github.com/mypkg").unwrap();
        assert!(pypi_satifisfies_requirement(&locked_data, &spec));
    }

    // Currently this test is missing from `good_satisiability`, so we test the specific windows case here
    // this should work an all supported platforms
    #[test]
    fn test_windows_absolute_path_handling() {
        // Mock locked datga
        let locked_data = PypiPackageData {
            name: "mypkg".parse().unwrap(),
            version: Version::from_str("0.1.0").unwrap(),
            url_or_path: UrlOrPath::Path(PathBuf::from_str("C:\\Users\\username\\mypkg").unwrap()),
            hash: None,
            requires_dist: vec![],
            requires_python: None,
            editable: false,
        };
        let spec = Requirement::from_str("mypkg @ file:///C:\\Users\\username\\mypkg").unwrap();
        // This should satisfy:
        assert!(pypi_satifisfies_requirement(&locked_data, &spec));
    }
}
