use std::path::Path;

use clap::{Parser, Subcommand};
use miette::IntoDiagnostic;
use pixi_config::{Config, GlobalConfigSource};
use pixi_core::{Workspace, environment::LockFileUsage};
use pixi_manifest::script::ScriptManifest;
use rattler_conda_types::Platform;

pub mod add;
pub mod init;
pub mod lock;
pub mod remove;
pub mod run;

/// Load the manifest of an existing script, failing with a `pixi script init`
/// hint when the file has no PEP 723 metadata block.
pub(crate) fn require_script(path: &Path) -> miette::Result<ScriptManifest> {
    if !path.exists() {
        return Err(miette::miette!("{} does not exist", path.display()));
    }
    ScriptManifest::from_path(path)?.ok_or_else(|| {
        miette::miette!(
            help = format!("Initialize it with `pixi script init {}`.", path.display()),
            "{} does not contain a PEP 723 metadata block",
            path.display()
        )
    })
}

/// Load the manifest of an existing Python file, adding a metadata block when
/// none is present. Only `pixi script init` creates new files.
pub(crate) fn require_or_init_script(
    path: &Path,
    config_source: &GlobalConfigSource,
    cli_config: Config,
) -> miette::Result<ScriptManifest> {
    let path = std::path::absolute(path).into_diagnostic()?;
    if !path.exists() {
        return Err(miette::miette!(
            help = format!("Create it with `pixi script init {}`.", path.display()),
            "{} does not exist",
            path.display()
        ));
    }
    if let Some(script) = ScriptManifest::from_path(&path)? {
        return Ok(script);
    }
    let root = path
        .parent()
        .expect("an absolute script path always has a parent");
    let config = Config::load_with(root, config_source).merge_config(cli_config);
    let channels = config
        .default_channels()
        .into_iter()
        .map(|channel| channel.to_string())
        .collect::<Vec<_>>();
    Ok(ScriptManifest::initialize(&path, &channels)?)
}

/// Construct the isolated workspace for a script, merging the global
/// configuration found next to the script with CLI overrides.
pub(crate) fn script_workspace(
    script: ScriptManifest,
    config_source: &GlobalConfigSource,
    cli_config: Config,
    platforms: Option<Vec<Platform>>,
) -> miette::Result<Workspace> {
    let root = script
        .path()
        .parent()
        .expect("an absolute script path always has a parent");
    let config = Config::load_with(root, config_source).merge_config(cli_config);
    let workspace = Workspace::from_script(script, config, platforms)?;
    for warning in workspace.warnings {
        tracing::warn!("{warning}");
    }
    Ok(workspace.value)
}

/// Warn about rich Pixi entries that could be expressed in portable script
/// metadata. Only commands that mutate the metadata nudge; `run` and `lock`
/// stay silent.
pub(crate) fn warn_portability(script: &ScriptManifest) -> miette::Result<()> {
    for warning in script.portability_warnings()? {
        tracing::warn!("{warning}");
    }
    Ok(())
}

/// Effective lock-file usage for a metadata edit (`add`/`remove`): an absent
/// sidecar lock is solved without being written, and `--frozen` proceeds
/// against the absent lock because editing does not need an environment.
pub(crate) fn edit_lock_file_usage(
    requested: LockFileUsage,
    lock_file_exists: bool,
) -> miette::Result<LockFileUsage> {
    lock_file_usage(requested, lock_file_exists, false)
}

/// Effective lock-file usage for `script run`, which cannot honor `--frozen`
/// without a lock to derive the environment from.
pub(crate) fn run_lock_file_usage(
    requested: LockFileUsage,
    lock_file_exists: bool,
) -> miette::Result<LockFileUsage> {
    lock_file_usage(requested, lock_file_exists, true)
}

fn lock_file_usage(
    requested: LockFileUsage,
    lock_file_exists: bool,
    frozen_requires_lock: bool,
) -> miette::Result<LockFileUsage> {
    if lock_file_exists {
        return Ok(requested);
    }
    let flag = match requested {
        LockFileUsage::Update | LockFileUsage::DryRun => return Ok(LockFileUsage::DryRun),
        LockFileUsage::Frozen if !frozen_requires_lock => return Ok(LockFileUsage::Frozen),
        LockFileUsage::Locked => "--locked",
        LockFileUsage::Frozen => "--frozen",
    };
    Err(miette::miette!(
        help = "Create one with `pixi script lock <PATH>`.",
        "no lock file exists for the script, but `{flag}` was requested"
    ))
}

/// Manage standalone scripts with inline dependency metadata.
#[derive(Debug, Parser)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Add conda or PyPI dependencies to a script.
    Add(add::Args),

    /// Add a PEP 723 metadata block to a new or existing script.
    Init(init::Args),

    /// Run a script in its isolated environment.
    Run(run::Args),

    /// Resolve a script environment and write its sidecar lock file.
    Lock(lock::Args),

    /// Remove dependencies from a script.
    Remove(remove::Args),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    match args.command {
        Command::Add(args) => add::execute(args).await,
        Command::Init(args) => init::execute(args).await,
        Command::Run(args) => run::execute(args).await,
        Command::Lock(args) => lock::execute(args).await,
        Command::Remove(args) => remove::execute(args).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_script_lock_is_solved_without_being_written() {
        assert_eq!(
            edit_lock_file_usage(LockFileUsage::Update, false).unwrap(),
            LockFileUsage::DryRun
        );
        assert_eq!(
            run_lock_file_usage(LockFileUsage::Update, false).unwrap(),
            LockFileUsage::DryRun
        );
        assert_eq!(
            edit_lock_file_usage(LockFileUsage::Update, true).unwrap(),
            LockFileUsage::Update
        );
        assert_eq!(
            run_lock_file_usage(LockFileUsage::Update, true).unwrap(),
            LockFileUsage::Update
        );
    }

    #[test]
    fn pinned_usage_without_a_lock_only_errs_when_an_environment_is_needed() {
        // Editing metadata does not need an environment, so `--frozen` may
        // proceed against the absent lock without creating one.
        assert_eq!(
            edit_lock_file_usage(LockFileUsage::Frozen, false).unwrap(),
            LockFileUsage::Frozen
        );
        assert!(edit_lock_file_usage(LockFileUsage::Locked, false).is_err());
        assert!(run_lock_file_usage(LockFileUsage::Frozen, false).is_err());
        assert!(run_lock_file_usage(LockFileUsage::Locked, false).is_err());
    }
}
