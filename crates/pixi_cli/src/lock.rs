use clap::Parser;
use miette::{Context, IntoDiagnostic};
use pixi_core::{
    WorkspaceLocator,
    environment::LockFileUsage,
    host::{cli_detector_consent, probe_conda_script},
    lock_file::{LockFileDerivedData, UpdateLockFileOptions},
};
use pixi_diff::{LockFileDiff, LockFileJsonDiff};
use pixi_manifest::CondaConstraints;
use pixi_spec::PixiSpec;
use rattler_conda_types::{
    ChannelConfig, MatchSpec, ParseMatchSpecError, ParseMatchSpecOptions, RepodataRevision,
};

use crate::cli_config::NoInstallConfig;
use crate::cli_config::ScriptWorkspaceConfig;

/// Solve environments and update the lock file without installing the
/// environments.
#[derive(Debug, Default, Parser)]
#[clap(arg_required_else_help = false)]
pub struct Args {
    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,

    #[clap(flatten)]
    pub workspace_config: ScriptWorkspaceConfig,

    #[clap(flatten)]
    pub config: pixi_config::ConfigCli,

    #[clap(flatten)]
    pub no_install_config: NoInstallConfig,

    /// Output the changes in JSON format.
    #[clap(long)]
    pub json: bool,

    /// Check if any changes have been made to the lock file.
    /// If yes, exit with a non-zero code.
    /// Implies `--dry-run`.
    #[clap(long)]
    pub check: bool,

    ///Compute the lock file without writing to disk.
    /// Implies `--no-install`.
    #[clap(long)]
    pub dry_run: bool,

    /// Temporarily constrain a conda package to the given match spec while
    /// locking, e.g. `--constraint 'openssl>=3.5.4'`.
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

pub async fn execute(args: Args) -> miette::Result<()> {
    let conda_script = match args.workspace_config.script.as_deref() {
        Some(path) => crate::conda_script::detect_with_fallback(path, false)?,
        None => None,
    };
    let mut workspace = if let Some(manifest) = conda_script {
        let root = manifest
            .path()
            .parent()
            .expect("an absolute script path always has a parent")
            .to_owned();
        let config = pixi_config::Config::try_load_with(&root, &args.config_source.source())?
            .merge_config(args.config.clone().into());
        let host =
            probe_conda_script(&manifest, &root, &config, cli_detector_consent(None)).await?;
        let pixi_manifest::WithWarnings {
            value: workspace,
            warnings,
        } = pixi_core::Workspace::from_conda_script(manifest, config, host)?;
        for warning in warnings {
            tracing::warn!("{warning}");
        }
        workspace
    } else {
        WorkspaceLocator::for_cli()
            .with_global_config_source(args.config_source.source())
            .with_search_start(args.workspace_config.workspace_locator_start())
            .with_cli_config(args.config.clone())
            .locate()
            .await?
    };

    // Apply backend override if provided (primarily for testing)
    if let Some(backend_override) = args
        .workspace_config
        .workspace_config
        .backend_override
        .clone()
    {
        workspace = workspace.with_backend_override(backend_override);
    }

    // Inject the temporary constraints. They only live in memory and take part
    // in both the satisfiability check and the solve.
    if !args.constraints.is_empty() {
        let constraints = parse_constraints(&args.constraints, &workspace.channel_config())?;
        workspace.workspace.value.extra_constraints = constraints;
    }

    // Update the lock file, and extract it from the derived data to drop additional resources
    // created for the solve.
    // Use the silent version here since update_lock_file() will display the warning.
    let original_lock_file = workspace.load_lock_file().await?.into_lock_file_or_empty();
    let progress = pixi_reporters::TopLevelProgress::from_global();
    // Scoped so the bars are cleared before the diff or the JSON is printed.
    let clear_progress = pixi_reporters::TopLevelProgress::clear_when_done(Some(&progress));
    // `--check` must be read-only: never write pixi.lock (even when it is missing).
    // Imply dry-run semantics so a missing/outdated lockfile surfaces as a check failure
    // without creating or rewriting the file.
    let read_only = args.check || args.dry_run;
    let (LockFileDerivedData { lock_file, .. }, lock_updated) = workspace
        .update_lock_file(
            Some(progress.clone()),
            UpdateLockFileOptions {
                lock_file_usage: if read_only {
                    LockFileUsage::DryRun
                } else {
                    LockFileUsage::Update
                },
                no_install: args.no_install_config.no_install || read_only,
                upgrade_lock_file_format: true,
                max_concurrent_solves: workspace.config().max_concurrent_solves(),
            },
        )
        .await?;
    drop(clear_progress);

    // Determine the diff between the old and new lock file.
    let diff = LockFileDiff::from_lock_files(&original_lock_file, &lock_file);

    // Format as json?
    if args.json {
        let diff = LockFileDiff::from_lock_files(&original_lock_file, &lock_file);
        let json_diff = LockFileJsonDiff::new(Some(workspace.named_environments()), diff);
        let json = serde_json::to_string_pretty(&json_diff).expect("failed to convert to json");
        println!("{json}");
    } else if read_only {
        if lock_updated {
            let prefix = if args.dry_run { "Dry-run: " } else { "" };
            eprintln!(
                "{}{prefix}lock file would be updated (not written to disk)",
                console::style(console::Emoji("i ", "i ")).blue()
            );
            diff.print()
                .into_diagnostic()
                .context("failed to print lock file diff")?;
        } else if args.check {
            eprintln!(
                "{}Lock-file was already up-to-date",
                console::style(console::Emoji("✔ ", "")).green()
            );
        } else {
            eprintln!(
                "{}Dry-run: lock file would not change",
                console::style(console::Emoji("i ", "i ")).blue()
            );
        }
    } else if lock_updated {
        eprintln!(
            "{}Updated lock file",
            console::style(console::Emoji("✔ ", "")).green()
        );
        diff.print()
            .into_diagnostic()
            .context("failed to print lock file diff")?;
    } else {
        eprintln!(
            "{}Lock-file was already up-to-date",
            console::style(console::Emoji("✔ ", "")).green()
        );
    }

    if args.check && lock_updated {
        miette::bail!("lock file not up-to-date with the workspace");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use clap::Parser;

    use super::Args;

    #[test]
    fn accepts_a_script_workspace() {
        let args = Args::try_parse_from(["lock", "--script", "example.py"]).unwrap();
        assert_eq!(
            args.workspace_config.script.as_deref(),
            Some(Path::new("example.py"))
        );
    }
}
