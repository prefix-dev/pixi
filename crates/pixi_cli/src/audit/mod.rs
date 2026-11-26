//! `pixi audit`: audits the workspace lock file for known vulnerabilities.

use std::{io::Write, path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand, ValueEnum};
use pixi_audit::{
    AuditReport, LockFileAuditError, Match, PackageEcosystem, ThreatSourceStatus, Verdict,
};
use pixi_manifest::AuditSeverity;
use rattler_lock::{LockFile, ParseCondaLockError};

use crate::cli_config::WorkspaceConfig;
use error::AuditCommandError;

mod clients;
mod error;
mod fix;
mod ignore;
mod policy;
mod render;

use render::{
    FixState, GuidanceOptions,
    annotations::{self, Level},
    sarif::{LockFileSource, SarifLog},
    table::{Column, DEFAULT_COLUMNS, TableLayout},
};

/// Audit the lock file of the workspace for known vulnerabilities.
///
/// Checks conda-forge packages with Basilisk and PyPI packages with OSV.
/// Packages from other channels, private indexes, Git, or paths are listed as
/// not audited. The audit never solves or installs; `--fix` solves once with
/// temporary constraints and replaces the lock file only when every finding
/// of the result passes the policy.
///
/// Exits with 1 when a vulnerability match fails the policy or the audit
/// cannot complete.
#[derive(Debug, Parser)]
pub struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,

    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    #[clap(flatten)]
    pub config: pixi_config::ConfigCli,

    /// Audit only this environment (repeatable).
    #[arg(long = "environment", short = 'e', value_name = "ENVIRONMENT")]
    pub environments: Vec<String>,

    /// Audit only this platform (repeatable).
    #[arg(long = "platform", short = 'p', value_name = "PLATFORM")]
    pub platforms: Vec<String>,

    /// Ignore this advisory id or alias, case-insensitively (repeatable).
    /// Added to the ignore rules of `[workspace.audit]`.
    #[arg(long, value_name = "ID")]
    pub ignore: Vec<String>,

    /// Ignore this advisory id or alias while no fixed release newer than the
    /// locked version is reported in
    /// the package's own ecosystem (repeatable).
    #[arg(long, value_name = "ID")]
    pub ignore_until_fixed: Vec<String>,

    /// Fail only for vulnerability matches at or above this severity.
    /// Matches with unknown severity always fail. Overrides `fail-on` of
    /// `[workspace.audit]`.
    #[arg(long, value_name = "SEVERITY", value_parser = parse_severity)]
    pub fail_on: Option<AuditSeverity>,

    /// Report vulnerability matches in this ecosystem without failing the
    /// audit (repeatable).
    #[arg(long, value_enum, value_name = "ECOSYSTEM")]
    pub warn_only_ecosystem: Vec<WarnOnlyEcosystem>,

    /// Update the lock file so that every vulnerability match passes the
    /// policy.
    ///
    /// Solves once with temporary constraints that exclude the affected
    /// versions, without installing anything or editing the manifest, then
    /// audits the whole result. The lock file is replaced only when every
    /// match then passes the policy and no package loses audit coverage. The
    /// constraints are not saved, so a later unconstrained solve can select
    /// an affected release again. Applies to all environments and platforms.
    #[arg(long, conflicts_with_all = ["environments", "platforms"])]
    pub fix: bool,

    /// Print the report as JSON (same as `--format json`).
    #[arg(long, conflicts_with = "format")]
    pub json: bool,

    /// The format of the report on stdout.
    #[arg(long, value_enum, default_value_t = Format::Table)]
    pub format: Format,

    /// Explain the terms and scoring of human-readable reports.
    #[arg(long)]
    pub explain: bool,

    /// The column to sort table rows by. Risk, severity, EPSS, and KEV sort
    /// highest first; other columns ascend.
    #[arg(long, value_enum, default_value = "risk", value_name = "COLUMN")]
    pub sort_by: Column,

    /// The columns of the table, in order (comma-separated).
    #[arg(long, value_enum, value_delimiter = ',', default_values_t = DEFAULT_COLUMNS, value_name = "COLUMNS")]
    pub columns: Vec<Column>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Ignore(ignore::Args),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum Format {
    /// A status summary and one row per vulnerability match.
    #[default]
    Table,
    /// Vulnerability matches grouped per package.
    ByPackage,
    /// Pretty-printed JSON.
    Json,
    /// SARIF 2.1.0, for GitHub code scanning and similar tools.
    Sarif,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum WarnOnlyEcosystem {
    CondaForge,
    Pypi,
}

impl From<WarnOnlyEcosystem> for PackageEcosystem {
    fn from(ecosystem: WarnOnlyEcosystem) -> Self {
        match ecosystem {
            WarnOnlyEcosystem::CondaForge => PackageEcosystem::CondaForge,
            WarnOnlyEcosystem::Pypi => PackageEcosystem::Pypi,
        }
    }
}

fn parse_severity(value: &str) -> Result<AuditSeverity, String> {
    value.parse()
}

/// Audits lock files with the settings of one invocation, so the candidate
/// of `--fix` is audited exactly like the original.
struct Auditor<'a> {
    policy: policy::EffectivePolicy,
    environments: &'a [String],
    platforms: &'a [String],
}

impl Auditor<'_> {
    async fn audit(
        &self,
        clients: &clients::AuditClients,
        lock_file: &LockFile,
        message: &str,
    ) -> Result<AuditReport, LockFileAuditError> {
        let message = format!(
            "{message} against {} and {}",
            clients.basilisk_url, clients.osv_url
        );
        pixi_progress::await_in_progress(message, |_| {
            pixi_audit::audit_lock_file(
                &clients.basilisk,
                &clients.osv,
                lock_file,
                self.environments,
                self.platforms,
                &self.policy.rules,
                clients.threat_sources(),
            )
        })
        .await
    }
}

fn warn_unavailable_threat_data(report: &AuditReport) {
    for (name, source) in [
        ("EPSS scores", &report.threat_data.epss),
        ("the KEV catalog", &report.threat_data.kev),
    ] {
        if let ThreatSourceStatus::Failed { error } = source {
            tracing::warn!(
                "{name} could not be loaded, so vulnerability matches are ranked without them: {error}"
            );
        }
    }
}

/// Reads the lock file, refusing a newer format instead of treating it as
/// missing.
fn read_lock_file(path: &std::path::Path) -> miette::Result<(String, LockFile)> {
    let contents = match fs_err::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => miette::bail!(
            help = "run `pixi lock` first",
            "the workspace has no lock file at '{}'",
            path.display()
        ),
        Err(source) => {
            return Err(AuditCommandError::Read {
                path: path.to_path_buf(),
                source,
            }
            .into());
        }
    };
    let lock_file = LockFile::from_str_with_base_directory(&contents, path.parent())
        .map_err(|err| match err {
            ParseCondaLockError::IncompatibleVersion {
                lock_file_version,
                max_supported_version,
            } => miette::miette!(
                help = "update pixi to read it",
                "the lock file uses format version {lock_file_version}, but this pixi supports up to {max_supported_version}"
            ),
            err => miette::miette!("failed to parse '{}': {err}", path.display()),
        })?;
    Ok((contents, lock_file))
}

pub async fn execute(args: Args) -> miette::Result<ExitCode> {
    let locator = crate::workspace_locator()
        .with_global_config_source(args.config_source.source())
        .with_search_start(args.workspace_config.workspace_locator_start())
        .with_cli_config(args.config.clone());
    if let Some(Command::Ignore(ignore_args)) = args.command {
        ignore::execute(ignore_args, locator).await?;
        return Ok(ExitCode::SUCCESS);
    }

    let format = if args.json { Format::Json } else { args.format };
    if args.explain && matches!(format, Format::Json | Format::Sarif) {
        miette::bail!("`--explain` is only available for the `table` and `by-package` formats");
    }

    let workspace = locator.locate().await?;
    let lock_file_path = workspace.lock_file_path();
    let inputs = fix::InputSnapshot::capture([
        lock_file_path.clone(),
        workspace.workspace.provenance.path.clone(),
    ])
    .map_err(|source| AuditCommandError::Read {
        path: workspace.root().to_path_buf(),
        source,
    })?;
    let (lock_file_contents, lock_file) = read_lock_file(&lock_file_path)?;

    let effective = policy::resolve(
        &workspace.workspace.value.workspace.audit,
        &policy::CliPolicy {
            ignore: args.ignore.clone(),
            ignore_until_fixed: args.ignore_until_fixed.clone(),
            fail_on: args.fail_on,
            warn_only: args
                .warn_only_ecosystem
                .iter()
                .map(|&e| PackageEcosystem::from(e))
                .collect(),
        },
        policy::today(),
    )?;
    for expired in &effective.expired {
        tracing::warn!(
            "the ignore rule {} expired on {} and no longer applies",
            expired.rule,
            expired.expired_at
        );
    }

    let clients = clients::build(workspace.config())?;
    let auditor = Auditor {
        policy: effective,
        environments: &args.environments,
        platforms: &args.platforms,
    };
    let report = auditor
        .audit(&clients, &lock_file, "Auditing the lock file")
        .await
        .map_err(AuditCommandError::from)?;
    warn_unavailable_threat_data(&report);

    let (report, fix_state, lock_file_contents, rejection) = if args.fix {
        match fix::run(
            &workspace,
            &auditor,
            &clients,
            &lock_file,
            &report,
            &lock_file_path,
            &inputs,
        )
        .await?
        {
            fix::FixOutcome::AlreadyPassing => {
                (report, FixState::NotRequested, lock_file_contents, None)
            }
            fix::FixOutcome::Applied {
                report: candidate,
                lock_file_contents,
                constraints,
            } => {
                eprintln!(
                    "{}Updated {} with temporary constraints: {}",
                    console::style(console::Emoji("✔ ", "")).green(),
                    lock_file_path.display(),
                    constraints
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                (candidate, FixState::Applied, lock_file_contents, None)
            }
            fix::FixOutcome::Rejected(rejection) => (
                report,
                FixState::NotApplied,
                lock_file_contents,
                Some(rejection),
            ),
        }
    } else {
        (report, FixState::NotRequested, lock_file_contents, None)
    };

    let options = GuidanceOptions {
        fix: fix_state,
        explain: args.explain,
    };
    let output = render_report(
        &report,
        format,
        &lock_file_path,
        &lock_file_contents,
        &TableLayout {
            columns: args.columns.clone(),
            sort_by: args.sort_by,
        },
        options,
    );
    let written = pixi_utils::io::ignore_broken_pipe(
        std::io::stdout()
            .lock()
            .write_all(output.as_bytes())
            .and_then(|()| std::io::stdout().flush()),
    );
    if let Err(err) = written {
        let err = miette::miette!("failed to write the report: {err}");
        return Err(if fix_state == FixState::Applied {
            err.wrap_err(format!(
                "the lock file at '{}' was already updated",
                lock_file_path.display()
            ))
        } else {
            err
        });
    }

    for rule in &report.unused_ignores {
        tracing::warn!("the ignore rule {rule} matched no vulnerability");
    }
    let policy = &auditor.policy.policy;
    if annotations::on_github_actions() {
        eprint!(
            "{}",
            annotations::github_annotations(&report, |vuln_match: &Match| {
                if policy.fails(vuln_match) {
                    Level::Error
                } else {
                    Level::Warning
                }
            })
        );
    }

    if let Some(rejection) = rejection {
        return Err(miette::Report::new(rejection)
            .wrap_err("`--fix` left the manifest and lock file unchanged"));
    }
    match policy.evaluate(&report) {
        Verdict::Fail { failing } => Err(miette::miette!(
            "{} the audit policy",
            if failing == 1 {
                "1 vulnerability match fails".to_string()
            } else {
                format!("{failing} vulnerability matches fail")
            }
        )),
        Verdict::BelowThreshold { rating, count } => {
            eprintln!(
                "{}{count} vulnerability {} below the '{rating}' severity threshold",
                console::style(console::Emoji("i ", "i ")).blue(),
                if count == 1 {
                    "match is"
                } else {
                    "matches are"
                }
            );
            Ok(ExitCode::SUCCESS)
        }
        Verdict::Pass => Ok(ExitCode::SUCCESS),
    }
}

fn render_report(
    report: &AuditReport,
    format: Format,
    lock_file_path: &std::path::Path,
    lock_file_contents: &str,
    layout: &TableLayout,
    options: GuidanceOptions,
) -> String {
    match format {
        Format::Json => {
            let mut json = serde_json::to_string_pretty(report).expect("reports serialize");
            json.push('\n');
            json
        }
        Format::Sarif => {
            let log = SarifLog::from_report(
                report,
                &LockFileSource {
                    path: lock_file_path,
                    contents: lock_file_contents,
                },
            );
            let mut json = serde_json::to_string_pretty(&log).expect("SARIF logs serialize");
            json.push('\n');
            json
        }
        Format::ByPackage => render::render_by_package(report, options),
        Format::Table => {
            render::table::render_table(report, &display_path(lock_file_path), layout, options)
        }
    }
}

/// The lock file path relative to the current directory when it is inside
/// it.
fn display_path(path: &std::path::Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(cwd).ok().map(PathBuf::from))
        .unwrap_or_else(|| path.to_path_buf())
        .display()
        .to_string()
}
