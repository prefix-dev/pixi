//! The `--with-constraint` option of `pixi lock`.

use std::{collections::HashSet, str::FromStr};

use clap::Parser;
use pixi_core::Workspace;
use pixi_manifest::CondaConstraints;
use pixi_spec::PixiSpec;
use rattler_conda_types::{
    ChannelConfig, MatchSpec, PackageName, ParseMatchSpecError, ParseMatchSpecOptions, RepodataRevision,
};
use rattler_lock::LockFile;

/// Temporary conda constraints that apply to a single invocation.
#[derive(Parser, Debug, Default, Clone)]
pub struct ConstraintArgs {
    /// Temporarily constrain a conda package to the given match spec, e.g.
    /// `--with-constraint 'openssl>=3.5.4'`.
    ///
    /// Behaves like an additional `[constraints]` entry in every feature, so
    /// it applies to all environments: it restricts the allowed versions of a
    /// package without adding it as a dependency. Environments whose locked
    /// packages violate the constraint are re-solved, keeping the other
    /// locked packages as preferences. The constraint is not written to the
    /// manifest. Can be specified multiple times.
    #[clap(long = "with-constraint", value_name = "MATCHSPEC")]
    pub constraints: Vec<String>,
}

impl ConstraintArgs {
    /// Injects the constraints into the in-memory workspace manifest. They
    /// take part in both the lock-file satisfiability check and the solve,
    /// but are never written to disk.
    pub fn apply(&self, workspace: &mut Workspace) -> Result<(), ConstraintParseError> {
        if self.constraints.is_empty() {
            return Ok(());
        }
        let constraints = parse_constraints(&self.constraints, &workspace.channel_config())?;
        workspace.workspace.value.extra_constraints = constraints;
        Ok(())
    }

    /// Tells the user which temporary constraints are applied, so that the
    /// resulting lock file changes can be traced back to them.
    pub fn print_applied(&self) {
        if self.constraints.is_empty() {
            return;
        }
        eprintln!(
            "{}Applying temporary constraints: {}",
            console::style(console::Emoji("i ", "i ")).blue(),
            self.constraints.join(", ")
        );
    }
}

/// Warns about temporary constraints whose conda package is absent from the
/// resulting lock file. The constraint may still have shaped the solve (e.g.
/// by moving a dependent to a version that no longer needs the package), but
/// a misspelled name or a package that comes from PyPI is the more common
/// cause.
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
                    conda_names.insert(conda.name().as_normalized().to_string());
                } else if let Some(pypi) = package.as_pypi() {
                    pypi_names.insert(pypi.name().clone());
                }
            }
        }
    }

    for warning in unmatched_constraint_warnings(constraints.names(), &conda_names, &pypi_names) {
        tracing::warn!("{warning}");
    }
}

/// Builds a warning for every constrained conda package that is absent from
/// the resulting lock file.
fn unmatched_constraint_warnings<'a>(
    constraint_names: impl IntoIterator<Item = &'a PackageName>,
    conda_names: &HashSet<String>,
    pypi_names: &HashSet<pep508_rs::PackageName>,
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

/// An error that occurs when parsing a `--with-constraint` argument.
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

/// Parses the `--with-constraint` arguments into conda constraints.
fn parse_constraints(
    constraints: &[String],
    channel_config: &ChannelConfig,
) -> Result<CondaConstraints, ConstraintParseError> {
    let mut result = CondaConstraints::default();
    for constraint in constraints {
        let spec = MatchSpec::from_str(
            constraint,
            ParseMatchSpecOptions::lenient().with_repodata_revision(RepodataRevision::V3),
        )
        .map_err(|source| ConstraintParseError::InvalidMatchSpec {
            constraint: constraint.clone(),
            source,
        })?;
        let Some(name) = spec.name.as_exact().cloned() else {
            return Err(ConstraintParseError::MissingPackageName(constraint.clone()));
        };
        let (_, nameless) = spec.into_nameless();
        result.insert(
            name,
            PixiSpec::from_nameless_matchspec(nameless, channel_config),
        );
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unmatched_constraint_warnings_normalize_pypi_names() {
        let conda_names = HashSet::from(["openssl".to_string()]);
        let pypi_names = HashSet::from([
            pep508_rs::PackageName::from_str("typing-extensions").unwrap(),
            pep508_rs::PackageName::from_str("zope.interface").unwrap(),
        ]);
        let constraint_names = [
            "openssl",
            "typing_extensions",
            "typing-extensions",
            "typing.extensions",
            "zope_interface",
            "missing",
        ]
        .map(|name| PackageName::from_str(name).unwrap());

        let warnings =
            unmatched_constraint_warnings(&constraint_names, &conda_names, &pypi_names);
        insta::assert_snapshot!(warnings.join("\n"), @r"
        the conda package 'typing_extensions' is absent from the resulting lock file, but the PyPI package 'typing-extensions' is locked: `--with-constraint` only applies to conda packages
        the conda package 'typing-extensions' is absent from the resulting lock file, but the PyPI package 'typing-extensions' is locked: `--with-constraint` only applies to conda packages
        the conda package 'typing.extensions' is absent from the resulting lock file, but the PyPI package 'typing-extensions' is locked: `--with-constraint` only applies to conda packages
        the conda package 'zope_interface' is absent from the resulting lock file, but the PyPI package 'zope-interface' is locked: `--with-constraint` only applies to conda packages
        the conda package 'missing' is absent from the resulting lock file
        ");
    }
}
