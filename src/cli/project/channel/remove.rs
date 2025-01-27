use crate::lock_file::UpdateMode;
use crate::{
    environment::{get_update_lock_file_and_prefix, LockFileUsage},
    UpdateLockFileOptions, Workspace,
};

use super::AddRemoveArgs;

pub async fn execute(args: AddRemoveArgs) -> miette::Result<()> {
    let mut project =
        Workspace::load_or_else_discover(args.project_config.manifest_path.as_deref())?
            .with_cli_config(args.clone().prefix_update_config.config);
    // Remove the channels from the manifest
    project
        .manifest
        .remove_channels(args.prioritized_channels(), &args.feature_name())?;

    // Try to update the lock-file without the removed channels
    get_update_lock_file_and_prefix(
        &project.default_environment(),
        UpdateMode::Revalidate,
        UpdateLockFileOptions {
            lock_file_usage: LockFileUsage::Update,
            no_install: args.prefix_update_config.no_install(),
            max_concurrent_solves: project.config().max_concurrent_solves(),
        },
    )
    .await?;
    project.save()?;

    // Report back to the user
    args.report("Removed", &project.channel_config())?;

    Ok(())
}
