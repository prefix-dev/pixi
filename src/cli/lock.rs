use crate::cli::cli_config::ProjectConfig;
use crate::diff::{LockFileDiff, LockFileJsonDiff};
use crate::environment::LockFileUsage;
use crate::lock_file::UpdateLockFileOptions;
use crate::Project;
use clap::Parser;
use miette::{Context, IntoDiagnostic};

/// Solve environment and update the lock file
#[derive(Debug, Parser)]
#[clap(arg_required_else_help = false)]
pub struct Args {
    #[clap(flatten)]
    pub project_config: ProjectConfig,

    /// Output the changes in JSON format.
    #[clap(long)]
    pub json: bool,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.project_config.manifest_path.as_deref())?;

    // Save the original lockfile to compare with the new one.
    let original_lockfile = project.get_lock_file().await?;

    let new_lockfile = project
        .update_lock_file(UpdateLockFileOptions {
            lock_file_usage: LockFileUsage::Update,
            no_install: false,
            max_concurrent_solves: project.config().max_concurrent_solves(),
        })
        .await?;

    // Determine the diff between the old and new lock-file.
    let diff = LockFileDiff::from_lock_files(&original_lockfile, &new_lockfile.lock_file);

    // Format as json?
    if args.json {
        let diff = LockFileDiff::from_lock_files(&original_lockfile, &new_lockfile.lock_file);
        let json_diff = LockFileJsonDiff::new(Some(&project), diff);
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
