//! The `--with-constraint` and `--with-pypi-constraint` options of `pixi lock`.

use std::{collections::HashSet, str::FromStr};

use clap::Parser;
use itertools::Itertools;
use pixi_core::Workspace;
use pixi_manifest::{CondaConstraints, PyPiConstraints};
use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName};
use pixi_spec::PixiSpec;
use rattler_conda_types::{
    ChannelConfig, MatchSpec, PackageName, PackageNameMatcher, ParseMatchSpecError,
    ParseMatchSpecOptions, RepodataRevision,
};
use rattler_lock::LockFile;

/// Temporary constraints for a single lock invocation.
#[derive(Parser, Debug, Default, Clone)]
pub struct ConstraintArgs {
    /// Temporarily constrain a conda package to the given match spec, e.g.
    /// `--with-constraint 'openssl>=3.5.4'`.
    ///
    /// Applies to all environments, including those with `no-default-feature = true`.
    /// Restricts the allowed versions of a package without adding it as a dependency.
    /// Environments whose locked packages violate the constraint are re-solved,
    /// keeping the other locked packages as preferences. The constraint is not written to the
    /// manifest. Can be specified multiple times.
    #[clap(long = "with-constraint", value_name = "MATCHSPEC", value_parser = parse_constraint)]
    pub constraints: Vec<MatchSpec>,

    /// Temporarily constrain a PyPI package to the given PEP 508 requirement,
    /// e.g. `--with-pypi-constraint 'urllib3>=2.2.2'`.
    ///
    /// Applies to all environments without adding dependencies or editing the
    /// manifest. Locked versions remain preferences. Can be specified multiple times.
    #[clap(long = "with-pypi-constraint", value_name = "REQUIREMENT", value_parser = parse_pypi_constraint)]
    pub pypi_constraints: Vec<pep508_rs::Requirement>,
}

impl ConstraintArgs {
    pub fn into_constraints(
        self,
        channel_config: &ChannelConfig,
    ) -> Result<(CondaConstraints, PyPiConstraints), ConstraintParseError> {
        let mut constraints = CondaConstraints::default();
        for spec in self.constraints {
            let (name, nameless) = spec.into_nameless();
            let PackageNameMatcher::Exact(name) = name else {
                return Err(ConstraintParseError::MissingPackageName(name.to_string()));
            };
            constraints.insert(
                name,
                PixiSpec::from_nameless_matchspec(nameless, channel_config),
            );
        }
        let mut pypi_constraints = PyPiConstraints::default();
        for requirement in self.pypi_constraints {
            let name = PypiPackageName::from_normalized(requirement.name.clone());
            let spec = PixiPypiSpec::try_from(requirement).expect("validated version constraint");
            pypi_constraints.insert(name, spec);
        }
        Ok((constraints, pypi_constraints))
    }

    /// Prints the temporary constraints to stderr.
    pub fn print_applied(&self) {
        if !self.constraints.is_empty() {
            eprintln!(
                "{}Applying temporary constraints: {}",
                console::style(console::Emoji("i ", "i ")).blue(),
                self.constraints.iter().format(", ")
            );
        }
        if !self.pypi_constraints.is_empty() {
            eprintln!(
                "{}Applying temporary PyPI constraints: {}",
                console::style(console::Emoji("i ", "i ")).blue(),
                self.pypi_constraints.iter().format(", ")
            );
        }
    }
}

/// Warns about constrained packages absent from the resulting lock file.
///
/// Absence does not imply the constraint had no effect: the solver may have
/// selected a dependent version that no longer needs the package.
pub fn warn_unmatched_constraints(workspace: &Workspace, lock_file: &LockFile) {
    let constraints = &workspace.workspace.value.extra_constraints;
    let pypi_constraints = &workspace.workspace.value.extra_pypi_constraints;
    if constraints.is_empty() && pypi_constraints.is_empty() {
        return;
    }

    let mut conda_names = HashSet::new();
    let mut pypi_names = HashSet::new();
    let mut conda_pypi_names = HashSet::new();
    for (_, environment) in lock_file.environments() {
        for (_, packages) in environment.packages_by_platform() {
            for package in packages {
                if let Some(conda) = package.as_conda() {
                    conda_names.insert(conda.name().as_normalized());
                    if !pypi_constraints.is_empty()
                        && let Some(record) = conda.record()
                    {
                        conda_pypi_names.extend(
                            record
                                .purls
                                .iter()
                                .flatten()
                                .filter(|purl| purl.package_type() == "pypi")
                                .filter_map(|purl| {
                                    purl.name().parse::<pep508_rs::PackageName>().ok()
                                }),
                        );
                    }
                } else if let Some(pypi) = package.as_pypi() {
                    pypi_names.insert(pypi.name());
                }
            }
        }
    }

    for warning in unmatched_constraint_warnings(constraints.names(), &conda_names, &pypi_names) {
        tracing::warn!("{warning}");
    }
    for name in pypi_constraints.names() {
        if !pypi_names.contains(name.as_normalized())
            && !conda_pypi_names.contains(name.as_normalized())
        {
            tracing::warn!(
                "the pypi package '{}' is absent from the resulting lock file",
                name.as_source()
            );
        }
    }
}

fn unmatched_constraint_warnings<'a>(
    constraint_names: impl IntoIterator<Item = &'a PackageName>,
    conda_names: &HashSet<&str>,
    pypi_names: &HashSet<&pep508_rs::PackageName>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    for name in constraint_names {
        let name = name.as_normalized();
        if conda_names.contains(name) {
            continue;
        }
        // PyPI names are normalized differently (`_` and `.` become `-`), so
        // `typing_extensions` must match a locked `typing-extensions`.
        let pypi_name = pep508_rs::PackageName::from_str(name)
            .ok()
            .filter(|pypi_name| pypi_names.contains(pypi_name));
        warnings.push(match pypi_name {
            Some(pypi_name) => format!(
                "the conda package '{name}' is absent from the resulting lock file, but the PyPI package '{pypi_name}' is locked: `--with-constraint` only applies to conda packages"
            ),
            None => format!("the conda package '{name}' is absent from the resulting lock file"),
        });
    }
    warnings
}

#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum ConstraintParseError {
    #[error("failed to parse constraint '{constraint}'")]
    InvalidMatchSpec {
        constraint: String,
        #[source]
        source: ParseMatchSpecError,
    },
    #[error("constraint '{0}' must specify an exact package name")]
    MissingPackageName(String),
    #[error("failed to parse pypi constraint '{constraint}'")]
    InvalidPypiRequirement {
        constraint: String,
        #[source]
        source: Box<pep508_rs::Pep508Error>,
    },
    #[error("pypi constraint '{0}' must be a version requirement without a url or extras")]
    UnsupportedPypiRequirement(String),
    #[error("pypi constraint '{0}' must specify a version requirement")]
    MissingPypiVersion(String),
}

fn parse_constraint(constraint: &str) -> Result<MatchSpec, ConstraintParseError> {
    let spec = MatchSpec::from_str(
        constraint,
        ParseMatchSpecOptions::lenient().with_repodata_revision(RepodataRevision::V3),
    )
    .map_err(|source| ConstraintParseError::InvalidMatchSpec {
        constraint: constraint.to_owned(),
        source,
    })?;
    if spec.name.as_exact().is_none() {
        return Err(ConstraintParseError::MissingPackageName(
            constraint.to_owned(),
        ));
    }
    Ok(spec)
}

fn parse_pypi_constraint(constraint: &str) -> Result<pep508_rs::Requirement, ConstraintParseError> {
    let requirement = pep508_rs::Requirement::from_str(constraint).map_err(|source| {
        ConstraintParseError::InvalidPypiRequirement {
            constraint: constraint.to_owned(),
            source: Box::new(source),
        }
    })?;
    if !requirement.extras.is_empty()
        || matches!(
            requirement.version_or_url,
            Some(pep508_rs::VersionOrUrl::Url(_))
        )
    {
        return Err(ConstraintParseError::UnsupportedPypiRequirement(
            constraint.to_owned(),
        ));
    }
    if !matches!(
        &requirement.version_or_url,
        Some(pep508_rs::VersionOrUrl::VersionSpecifier(specifiers)) if !specifiers.is_empty()
    ) {
        return Err(ConstraintParseError::MissingPypiVersion(
            constraint.to_owned(),
        ));
    }
    Ok(requirement)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constraints_require_exact_package_names() {
        for constraint in ["* >=2", "foo* >=2", "foo >=>2"] {
            let err = ConstraintArgs::try_parse_from(["pixi", "--with-constraint", constraint])
                .unwrap_err();
            assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
        }
    }

    #[test]
    fn pypi_constraints_only_restrict_versions() {
        for constraint in [
            "foo @ https://example.com/foo-1.0.0-py3-none-any.whl",
            "foo[extra]<2",
            "foo<<2",
            "foo",
            "foo; sys_platform == 'linux'",
        ] {
            let err =
                ConstraintArgs::try_parse_from(["pixi", "--with-pypi-constraint", constraint])
                    .unwrap_err();
            assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
        }
    }

    #[test]
    fn unmatched_constraint_warnings_normalize_pypi_names() {
        let conda_names = HashSet::from(["openssl"]);
        let pypi_packages = [
            pep508_rs::PackageName::from_str("typing-extensions").unwrap(),
            pep508_rs::PackageName::from_str("zope.interface").unwrap(),
        ];
        let pypi_names = pypi_packages.iter().collect();
        let constraint_names = [
            "openssl",
            "typing_extensions",
            "typing-extensions",
            "typing.extensions",
            "zope_interface",
            "missing",
        ]
        .map(|name| PackageName::from_str(name).unwrap());

        let warnings = unmatched_constraint_warnings(&constraint_names, &conda_names, &pypi_names);
        insta::assert_snapshot!(warnings.join("\n"), @r"
        the conda package 'typing_extensions' is absent from the resulting lock file, but the PyPI package 'typing-extensions' is locked: `--with-constraint` only applies to conda packages
        the conda package 'typing-extensions' is absent from the resulting lock file, but the PyPI package 'typing-extensions' is locked: `--with-constraint` only applies to conda packages
        the conda package 'typing.extensions' is absent from the resulting lock file, but the PyPI package 'typing-extensions' is locked: `--with-constraint` only applies to conda packages
        the conda package 'zope_interface' is absent from the resulting lock file, but the PyPI package 'zope-interface' is locked: `--with-constraint` only applies to conda packages
        the conda package 'missing' is absent from the resulting lock file
        ");
    }
}
