//! The `--with-constraint` option of `pixi lock`.

use std::{collections::HashSet, str::FromStr};

use clap::Parser;
use itertools::Itertools;
use pixi_core::Workspace;
use pixi_manifest::CondaConstraints;
use pixi_spec::PixiSpec;
use rattler_conda_types::{
    ChannelConfig, MatchSpec, PackageName, PackageNameMatcher, ParseMatchSpecError,
    ParseMatchSpecOptions, RepodataRevision,
};
use rattler_lock::LockFile;

/// Temporary conda constraints that apply to a single invocation.
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
}

impl ConstraintArgs {
    pub fn into_constraints(
        self,
        channel_config: &ChannelConfig,
    ) -> Result<CondaConstraints, ConstraintParseError> {
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
        Ok(constraints)
    }

    /// Prints the temporary constraints to stderr.
    pub fn print_applied(&self) {
        if self.constraints.is_empty() {
            return;
        }
        eprintln!(
            "{}Applying temporary constraints: {}",
            console::style(console::Emoji("i ", "i ")).blue(),
            self.constraints.iter().format(", ")
        );
    }
}

/// Warns about constrained packages absent from the resulting lock file.
///
/// Absence does not imply the constraint had no effect: the solver may have
/// selected a dependent version that no longer needs the package.
pub fn warn_unmatched_constraints(workspace: &Workspace, lock_file: &LockFile) {
    let constraints = &workspace.workspace.value.extra_constraints;
    if constraints.is_empty() {
        return;
    }

    let mut conda_names = HashSet::new();
    let mut pypi_names = HashSet::new();
    for (_, environment) in lock_file.environments() {
        for (_, packages) in environment.packages_by_platform() {
            for package in packages {
                if let Some(conda) = package.as_conda() {
                    conda_names.insert(conda.name().as_normalized());
                } else if let Some(pypi) = package.as_pypi() {
                    pypi_names.insert(pypi.name());
                }
            }
        }
    }

    for warning in unmatched_constraint_warnings(constraints.names(), &conda_names, &pypi_names) {
        tracing::warn!("{warning}");
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
