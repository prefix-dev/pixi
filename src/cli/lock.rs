use clap::Parser;

use crate::{
    cli::cli_config::WorkspaceConfig, environment::LockFileUsage, lock_file::UpdateLockFileOptions,
    WorkspaceLocator,
};

/// Solve environment and update the lock file
#[derive(Debug, Parser)]
#[clap(arg_required_else_help = false)]
pub struct Args {
    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    workspace
        .update_lock_file(UpdateLockFileOptions {
            lock_file_usage: LockFileUsage::Update,
            no_install: true,
            max_concurrent_solves: workspace.config().max_concurrent_solves(),
        })
        .await
        .map(|_| ())
}
