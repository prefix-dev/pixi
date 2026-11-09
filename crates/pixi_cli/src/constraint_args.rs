//! The `--with-constraint` and `--with-pypi-constraint` options shared by
//! `pixi lock` and `pixi update`.

use std::str::FromStr;

use clap::Parser;
use pixi_core::Workspace;
use pixi_manifest::{CondaConstraints, PyPiConstraints};
use pixi_pypi_spec::{PixiPypiSource, PixiPypiSpec, PypiPackageName, VersionOrStar};
use pixi_spec::PixiSpec;
use rattler_conda_types::{
    ChannelConfig, MatchSpec, ParseMatchSpecError, ParseMatchSpecOptions, RepodataRevision,
};

/// Temporary conda constraints that apply to a single invocation.
#[derive(Parser, Debug, Default, Clone)]
pub struct ConstraintArgs {
    /// Temporarily constrain a conda package to the given match spec, e.g.
    /// `--with-constraint 'openssl>=3.5.4'`.
    ///
    /// Behaves like an additional entry in the `[constraints]` table that
    /// applies to all environments: it restricts the allowed versions of a
    /// package without adding it as a dependency. Environments whose locked
    /// packages violate the constraint are re-solved, keeping the other
    /// locked packages as preferences. The constraint is not written to the
    /// manifest. Can be specified multiple times.
    #[clap(long = "with-constraint", value_name = "MATCHSPEC")]
    pub constraints: Vec<String>,

    /// Temporarily constrain a PyPI package to the given PEP 508 requirement,
    /// e.g. `--with-pypi-constraint 'urllib3>=2.2.2'`.
    ///
    /// Behaves like an additional entry in the `[pypi-constraints]` table
    /// that applies to all environments: it restricts the allowed versions of
    /// a package without adding it as a dependency. Environments whose locked
    /// packages violate the constraint are re-solved, keeping the other
    /// locked packages as preferences. The constraint is not written to the
    /// manifest. Can be specified multiple times.
    #[clap(long = "with-pypi-constraint", value_name = "REQUIREMENT")]
    pub pypi_constraints: Vec<String>,
}

impl ConstraintArgs {
    /// Injects the constraints into the in-memory workspace manifest. They
    /// take part in both the lock-file satisfiability check and the solve,
    /// but are never written to disk.
    pub fn apply(&self, workspace: &mut Workspace) -> Result<(), ConstraintParseError> {
        if !self.constraints.is_empty() {
            workspace.workspace.value.extra_constraints =
                parse_constraints(&self.constraints, &workspace.channel_config())?;
        }
        if !self.pypi_constraints.is_empty() {
            workspace.workspace.value.extra_pypi_constraints =
                parse_pypi_constraints(&self.pypi_constraints)?;
        }
        Ok(())
    }

    /// Tells the user which temporary constraints are applied, so that the
    /// resulting lock file changes can be traced back to them.
    pub fn print_applied(&self) {
        if !self.constraints.is_empty() {
            eprintln!(
                "{}Applying temporary constraints: {}",
                console::style(console::Emoji("i ", "i ")).blue(),
                self.constraints.join(", ")
            );
        }
        if !self.pypi_constraints.is_empty() {
            eprintln!(
                "{}Applying temporary PyPI constraints: {}",
                console::style(console::Emoji("i ", "i ")).blue(),
                self.pypi_constraints.join(", ")
            );
        }
    }
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
    #[error("failed to parse pypi constraint '{constraint}'")]
    InvalidPypiRequirement {
        constraint: String,
        #[source]
        source: Box<pep508_rs::Pep508Error>,
    },
    #[error("pypi constraint '{0}' must be a version requirement without a url or extras")]
    UnsupportedPypiRequirement(String),
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

/// Parses the `--with-pypi-constraint` arguments into PyPI constraints.
///
/// Like the `[pypi-constraints]` table, a constraint can only restrict the
/// allowed versions, optionally with environment markers.
fn parse_pypi_constraints(constraints: &[String]) -> Result<PyPiConstraints, ConstraintParseError> {
    let mut result = PyPiConstraints::default();
    for constraint in constraints {
        let requirement = pep508_rs::Requirement::from_str(constraint).map_err(|source| {
            ConstraintParseError::InvalidPypiRequirement {
                constraint: constraint.clone(),
                source: Box::new(source),
            }
        })?;
        let version = match requirement.version_or_url {
            None => VersionOrStar::Star,
            Some(pep508_rs::VersionOrUrl::VersionSpecifier(version)) => version.into(),
            Some(pep508_rs::VersionOrUrl::Url(_)) => {
                return Err(ConstraintParseError::UnsupportedPypiRequirement(
                    constraint.clone(),
                ));
            }
        };
        if !requirement.extras.is_empty() {
            return Err(ConstraintParseError::UnsupportedPypiRequirement(
                constraint.clone(),
            ));
        }
        result.insert(
            PypiPackageName::from_normalized(requirement.name),
            PixiPypiSpec::with_extras_and_markers(
                PixiPypiSource::Registry {
                    version,
                    index: None,
                },
                Vec::new(),
                requirement.marker,
            ),
        );
    }
    Ok(result)
}
