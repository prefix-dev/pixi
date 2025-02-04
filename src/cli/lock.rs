use clap::Parser;

use crate::cli::cli_config::ProjectConfig;
use crate::environment::LockFileUsage;
use crate::lock_file::UpdateLockFileOptions;
use crate::Project;

/// Solve environment and update the lock file
#[derive(Debug, Parser)]
#[clap(arg_required_else_help = false)]
pub struct Args {
    #[clap(flatten)]
    pub project_config: ProjectConfig,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.project_config.manifest_path.as_deref())?;

    project
        .update_lock_file(UpdateLockFileOptions {
            lock_file_usage: LockFileUsage::Update,
            no_install: true,
            max_concurrent_solves: project.config().max_concurrent_solves(),
        })
        .await
        .map(|_| ())
}
