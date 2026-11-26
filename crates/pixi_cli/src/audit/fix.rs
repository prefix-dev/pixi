//! `--fix`: one constrained solve, one complete audit of the candidate, and a
//! single atomic lock file replacement when the candidate passes.
//!
//! The order is fixed: derive temporary constraints for the policy-failing
//! matches of the audited lock file, solve once from exactly that lock file
//! without installing anything, audit the whole candidate, check the policy
//! and audit coverage, check that the inputs did not change, then replace the
//! lock file. Nothing is written before the candidate passes, the manifest is
//! never edited, and project and solve environments are never installed.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use itertools::Itertools;
use miette::Diagnostic;
use pixi_audit::{
    AuditReport, Coverage, CoverageRegression, LockFileAuditError, ManifestRequirement,
    PackageName, PlanBlocker, TemporaryConstraint,
};
use pixi_core::{
    Workspace,
    environment::LockFileUsage,
    lock_file::{UpdateLockFileError, UpdateLockFileOptions},
};
use pixi_manifest::{CondaConstraints, FeaturesExt, HasWorkspaceManifest, PyPiConstraints};
use pixi_pypi_spec::{PixiPypiSource, PixiPypiSpec, PypiPackageName, VersionOrStar};
use pixi_spec::PixiSpec;
use rattler_conda_types::NamelessMatchSpec;
use rattler_lock::LockFile;
use thiserror::Error;

use super::{Auditor, clients::AuditClients};

/// The bytes of the files `--fix` depends on, captured before solving so
/// changes made while it runs are detected before the lock file is replaced.
///
/// This is an optimistic check, not a compare-and-swap: a writer that does
/// not cooperate can still change a file between the check and the rename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InputSnapshot(Vec<(PathBuf, Option<Vec<u8>>)>);

impl InputSnapshot {
    pub fn capture(paths: impl IntoIterator<Item = PathBuf>) -> std::io::Result<Self> {
        paths
            .into_iter()
            .map(|path| {
                let contents = match fs_err::read(&path) {
                    Ok(contents) => Some(contents),
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
                    Err(err) => return Err(err),
                };
                Ok((path, contents))
            })
            .collect::<std::io::Result<_>>()
            .map(Self)
    }

    fn changed_paths(&self) -> std::io::Result<Vec<PathBuf>> {
        let current = Self::capture(self.0.iter().map(|(path, _)| path.clone()))?;
        Ok(self
            .0
            .iter()
            .zip(&current.0)
            .filter(|(before, after)| before != after)
            .map(|((path, _), _)| path.clone())
            .collect())
    }
}

/// Why `--fix` did not replace the lock file.
#[derive(Debug, Error, Diagnostic)]
pub(super) enum FixRejected {
    #[error(
        "no temporary constraint can resolve every vulnerability match that fails the policy:\n{}",
        .0.iter().map(|blocker| format!("  - {blocker}")).join("\n")
    )]
    #[diagnostic(help(
        "update or replace the affected dependencies by hand, or record an accepted risk with `pixi audit ignore`"
    ))]
    Blocked(Vec<PlanBlocker>),

    #[error("solving with the temporary constraints failed")]
    Solve(
        #[source]
        #[diagnostic_source]
        UpdateLockFileError,
    ),

    #[error("auditing the candidate lock file failed")]
    CandidateAudit(#[source] LockFileAuditError),

    #[error(
        "{} in the candidate lock file still fail the policy:\n{}",
        if .0.len() == 1 { "1 vulnerability match".to_string() } else { format!("{} vulnerability matches", .0.len()) },
        .0.iter().map(|line| format!("  - {line}")).join("\n")
    )]
    #[diagnostic(help(
        "the solver may have selected another affected release, or a dependency may keep the package at an affected version"
    ))]
    Remaining(Vec<String>),

    #[error(
        "the candidate lock file audits less than the current one:\n{}",
        .0.iter().map(|line| format!("  - {line}")).join("\n")
    )]
    CoverageLost(Vec<String>),

    #[error(
        "{} changed while `--fix` was running",
        .0.iter().map(|path| path.display().to_string()).join(", ")
    )]
    #[diagnostic(help("run `pixi audit --fix` again"))]
    InputsChanged(Vec<PathBuf>),
}

/// What `--fix` did.
pub(super) enum FixOutcome {
    /// The audited lock file already passes the policy, so nothing was solved.
    AlreadyPassing,
    /// The lock file was replaced. The report describes the new lock file.
    Applied {
        report: AuditReport,
        lock_file_contents: String,
        constraints: Vec<TemporaryConstraint>,
    },
    /// Nothing was written.
    Rejected(FixRejected),
}

/// Every version requirement the manifest places on a package, so the
/// temporary constraints leave room for them.
fn manifest_requirements(workspace: &Workspace) -> Vec<ManifestRequirement> {
    let mut requirements = BTreeSet::new();
    let manifest = &workspace.workspace_manifest().workspace;
    for environment in workspace.environments() {
        let platforms = environment
            .platforms()
            .into_iter()
            .filter_map(|name| manifest.platform_by_name(&name).cloned())
            .collect_vec();
        for platform in std::iter::once(None).chain(platforms.iter().map(Some)) {
            let conda = environment.combined_dependencies(platform);
            let constraints = environment.combined_constraints(platform);
            for (name, spec) in conda.iter_specs().chain(constraints.iter_specs()) {
                if let Some(version) = spec.as_version_spec() {
                    requirements.insert((PackageName::Conda(name.clone()), version.to_string()));
                }
            }
            let pypi = environment.pypi_dependencies(platform);
            let pypi_constraints = environment.combined_pypi_constraints(platform);
            for (name, spec) in pypi.iter_specs().chain(pypi_constraints.iter_specs()) {
                if let (Some(VersionOrStar::Version(version)), Ok(name)) = (
                    spec.as_version(),
                    PackageName::pypi(name.as_normalized().as_ref()),
                ) {
                    requirements.insert((name, version.to_string()));
                }
            }
        }
    }
    requirements
        .into_iter()
        .map(|(package, spec)| ManifestRequirement { package, spec })
        .collect()
}

/// The planned constraints as typed workspace constraints.
fn workspace_constraints(
    workspace: &Workspace,
    constraints: &[TemporaryConstraint],
) -> miette::Result<(CondaConstraints, PyPiConstraints)> {
    let channel_config = workspace.channel_config();
    let mut conda = CondaConstraints::default();
    let mut pypi = PyPiConstraints::default();
    for constraint in constraints {
        match constraint {
            TemporaryConstraint::Conda { name, version, .. } => conda.insert(
                name.clone(),
                PixiSpec::from_nameless_matchspec(
                    NamelessMatchSpec {
                        version: Some(version.clone()),
                        ..NamelessMatchSpec::default()
                    },
                    &channel_config,
                ),
            ),
            TemporaryConstraint::Pypi {
                name, specifiers, ..
            } => {
                let name = name
                    .as_str()
                    .parse::<pep508_rs::PackageName>()
                    .map_err(|err| miette::miette!("invalid PyPI package name '{name}': {err}"))?;
                let specifiers =
                    pixi_uv_conversions::to_version_specifiers(specifiers).map_err(|err| {
                        miette::miette!("invalid PyPI constraint '{specifiers}': {err}")
                    })?;
                pypi.insert(
                    PypiPackageName::from_normalized(name),
                    PixiPypiSpec::new(PixiPypiSource::Registry {
                        version: VersionOrStar::Version(specifiers),
                        index: None,
                    }),
                );
            }
        }
    }
    Ok((conda, pypi))
}

fn describe_regression(regression: &CoverageRegression) -> String {
    let (key, source, what) = match regression {
        CoverageRegression::NewlyUnaudited { key, source } => {
            (key, source, "is new and cannot be audited")
        }
        CoverageRegression::LostCoverage { key, source } => {
            (key, source, "would no longer be audited")
        }
    };
    format!(
        "{} from {source} in environment '{}' for '{}' {what}",
        key.name, key.environment, key.platform
    )
}

/// Runs `--fix` on top of `report`, the audit of `baseline`.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run(
    workspace: &Workspace,
    auditor: &Auditor<'_>,
    clients: &AuditClients,
    baseline: &LockFile,
    report: &AuditReport,
    lock_file_path: &Path,
    inputs: &InputSnapshot,
) -> miette::Result<FixOutcome> {
    let policy = &auditor.policy.policy;
    if policy.evaluate(report).passes() {
        return Ok(FixOutcome::AlreadyPassing);
    }

    let plan =
        match pixi_audit::remediation::plan(report, policy, &manifest_requirements(workspace)) {
            Ok(plan) => plan,
            Err(blockers) => return Ok(FixOutcome::Rejected(FixRejected::Blocked(blockers))),
        };
    eprintln!(
        "{}Solving with temporary constraints: {}",
        console::style(console::Emoji("i ", "i ")).blue(),
        plan.constraints.iter().format(", ")
    );

    let (conda, pypi) = workspace_constraints(workspace, &plan.constraints)?;
    let constrained = workspace
        .clone()
        .with_extra_constraints(conda)
        .with_extra_pypi_constraints(pypi);
    let progress = pixi_reporters::TopLevelProgress::from_global();
    let clear_progress = pixi_reporters::TopLevelProgress::clear_when_done(Some(&progress));
    let solved = constrained
        .update_lock_file_from_lock_file(
            Some(progress.clone()),
            UpdateLockFileOptions {
                lock_file_usage: LockFileUsage::DryRun,
                no_install: true,
                upgrade_lock_file_format: false,
                max_concurrent_solves: constrained.config().max_concurrent_solves(),
            },
            baseline.clone(),
        )
        .await;
    drop(clear_progress);
    let candidate = match solved {
        Ok((derived, _)) => derived.into_lock_file(),
        Err(err) => return Ok(FixOutcome::Rejected(FixRejected::Solve(err))),
    };

    let candidate_report = match auditor
        .audit(clients, &candidate, "Auditing the candidate lock file")
        .await
    {
        Ok(report) => report,
        Err(err) => return Ok(FixOutcome::Rejected(FixRejected::CandidateAudit(err))),
    };
    super::warn_unavailable_threat_data(&candidate_report);

    let remaining: Vec<String> = policy
        .failing(&candidate_report)
        .map(|m| {
            format!(
                "{} in {} {} ({})",
                m.id,
                m.package.name,
                m.package.version,
                m.environments.join(", ")
            )
        })
        .collect();
    if !remaining.is_empty() {
        return Ok(FixOutcome::Rejected(FixRejected::Remaining(remaining)));
    }

    let regressions = Coverage::from_lock_file(baseline)
        .and_then(|before| Ok(before.regressions(&Coverage::from_lock_file(&candidate)?)))
        .map_err(|err| miette::miette!("{err}"))?;
    if !regressions.is_empty() {
        return Ok(FixOutcome::Rejected(FixRejected::CoverageLost(
            regressions.iter().map(describe_regression).collect(),
        )));
    }

    let contents = candidate
        .render_to_string()
        .map_err(|err| miette::miette!("failed to serialize the candidate lock file: {err}"))?;
    let changed = inputs
        .changed_paths()
        .map_err(|err| miette::miette!("failed to re-read the inputs of `--fix`: {err}"))?;
    if !changed.is_empty() {
        return Ok(FixOutcome::Rejected(FixRejected::InputsChanged(changed)));
    }
    pixi_utils::atomic_write::replace_atomically(lock_file_path, &contents).map_err(|err| {
        miette::miette!(
            "failed to replace '{}', which is unchanged: {err}",
            lock_file_path.display()
        )
    })?;

    Ok(FixOutcome::Applied {
        report: candidate_report,
        lock_file_contents: contents,
        constraints: plan.constraints,
    })
}
