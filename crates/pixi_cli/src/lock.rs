use clap::Parser;
use miette::{Context, IntoDiagnostic};
use pixi_core::{
    WorkspaceLocator,
    diff::{LockFileDiff, LockFileJsonDiff},
    environment::LockFileUsage,
    lock_file::{LockFileDerivedData, UpdateLockFileOptions},
};

use crate::cli_config::NoInstallConfig;

use crate::cli_config::WorkspaceConfig;

/// Solve environment and update the lock file without installing the
/// environments.
#[derive(Debug, Parser)]
#[clap(arg_required_else_help = false)]
pub struct Args {
    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    #[clap(flatten)]
    pub no_install_config: NoInstallConfig,

    /// Output the changes in JSON format.
    #[clap(long)]
    pub json: bool,

    /// Check if any changes have been made to the lock file.
    /// If yes, exit with a non-zero code.
    #[clap(long)]
    pub check: bool,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    // Update the lock-file, and extract it from the derived data to drop additional resources
    // created for the solve.
    let original_lock_file = workspace.load_lock_file().await?;
    let (LockFileDerivedData { lock_file, .. }, lock_updated) = workspace
        .update_lock_file(UpdateLockFileOptions {
            lock_file_usage: LockFileUsage::Update,
            no_install: args.no_install_config.no_install,
            max_concurrent_solves: workspace.config().max_concurrent_solves(),
        })
        .await?;

    // Determine the diff between the old and new lock-file.
    let diff = LockFileDiff::from_lock_files(&original_lock_file, &lock_file);

    // Format as json?
    if args.json {
        let diff = LockFileDiff::from_lock_files(&original_lock_file, &lock_file);
        let json_diff = LockFileJsonDiff::new(Some(&workspace), diff);
        let json = serde_json::to_string_pretty(&json_diff).expect("failed to convert to json");
        println!("{}", json);
    } else if lock_updated {
        eprintln!(
            "{}Updated lock-file",
            console::style(console::Emoji("✔ ", "")).green()
        );
        diff.print()
            .into_diagnostic()
            .context("failed to print lock-file diff")?;
    } else {
        eprintln!(
            "{}Lock-file was already up-to-date",
            console::style(console::Emoji("✔ ", "")).green()
        );
    }

    // Return with a non-zero exit code if `--check` has been passed and the lock
    // file has been updated
    if args.check && !diff.is_empty() {
        std::process::exit(1);
    }

    Ok(())
}
