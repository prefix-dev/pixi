//! The `--constraint` option shared by `pixi lock` and `pixi update`.

use clap::Parser;
use pixi_core::Workspace;
use pixi_manifest::CondaConstraints;
use pixi_spec::PixiSpec;
use rattler_conda_types::{
    ChannelConfig, MatchSpec, ParseMatchSpecError, ParseMatchSpecOptions, RepodataRevision,
};

/// Temporary conda constraints that apply to a single invocation.
#[derive(Parser, Debug, Default, Clone)]
pub struct ConstraintArgs {
    /// Temporarily constrain a conda package to the given match spec, e.g.
    /// `--constraint 'openssl>=3.5.4'`.
    ///
    /// Behaves like an additional entry in the `[constraints]` table that
    /// applies to all environments: it restricts the allowed versions of a
    /// package without adding it as a dependency. Environments whose locked
    /// packages violate the constraint are re-solved, keeping the other
    /// locked packages as preferences. The constraint is not written to the
    /// manifest. Can be specified multiple times.
    #[clap(long = "constraint", value_name = "MATCHSPEC")]
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
}

/// An error that occurs when parsing a `--constraint` argument.
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

/// Parses the `--constraint` arguments into conda constraints.
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
