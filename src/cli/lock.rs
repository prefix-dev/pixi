use clap::Parser;
use miette::{Context, IntoDiagnostic};

use crate::{
    cli::cli_config::WorkspaceConfig,
    diff::{LockFileDiff, LockFileJsonDiff},
    environment::LockFileUsage,
    lock_file::UpdateLockFileOptions,
    WorkspaceLocator,
};

/// Solve environment and update the lock file
#[derive(Debug, Parser)]
#[clap(arg_required_else_help = false)]
pub struct Args {
    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    /// Output the changes in JSON format.
    #[clap(long)]
    pub json: bool,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    // Save the original lockfile to compare with the new one.
    let original_lock_file = workspace.load_lock_file().await?;
    let new_lock_file = workspace
        .update_lock_file(UpdateLockFileOptions {
            lock_file_usage: LockFileUsage::Update,
            no_install: false,
            max_concurrent_solves: workspace.config().max_concurrent_solves(),
        })
        .await?;

    // Determine the diff between the old and new lock-file.
    let diff = LockFileDiff::from_lock_files(&original_lock_file, &new_lock_file.lock_file);

    // Format as json?
    if args.json {
        let diff = LockFileDiff::from_lock_files(&original_lock_file, &new_lock_file.lock_file);
        let json_diff = LockFileJsonDiff::new(Some(&workspace), diff);
        let json = serde_json::to_string_pretty(&json_diff).expect("failed to convert to json");
        println!("{}", json);
    } else if diff.is_empty() {
        eprintln!(
            "{}Lock-file was already up-to-date",
            console::style(console::Emoji("✔ ", "")).green()
        );
    } else {
        diff.print()
            .into_diagnostic()
            .context("failed to print lock-file diff")?;
    }

    Ok(())
}
