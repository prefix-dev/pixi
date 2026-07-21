use std::path::Path;

use clap::{Parser, Subcommand};
use pixi_config::{Config, GlobalConfigSource};
use pixi_core::{Workspace, environment::LockFileUsage};
use pixi_manifest::script::ScriptManifest;

pub mod init;
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

/// Construct the isolated workspace for a script, merging the global
/// configuration found next to the script with CLI overrides.
pub(crate) fn script_workspace(
    script: ScriptManifest,
    config_source: &GlobalConfigSource,
    cli_config: Config,
) -> miette::Result<Workspace> {
    let root = script
        .path()
        .parent()
        .expect("an absolute script path always has a parent");
    let config = Config::load_with(root, config_source).merge_config(cli_config);
    let workspace = Workspace::from_script(script, config)?;
    for warning in workspace.warnings {
        tracing::warn!("{warning}");
    }
    Ok(workspace.value)
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
    /// Add a PEP 723 metadata block to a new or existing script.
    Init(init::Args),

    /// Run a script in its isolated environment.
    Run(run::Args),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    match args.command {
        Command::Init(args) => init::execute(args).await,
        Command::Run(args) => run::execute(args).await,
    }
}
