use pixi_core::{
    UpdateLockFileOptions, WorkspaceLocator,
    environment::{LockFileUsage, get_update_lock_file_and_prefix},
    lock_file::ReinstallPackages,
};
use miette::IntoDiagnostic;

use super::AddRemoveArgs;

pub async fn execute(args: AddRemoveArgs) -> miette::Result<()> {
    let mut workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?
        .with_cli_config(args.config.clone())
        .modify()?;

    // Remove the channels from the manifest
    workspace
        .manifest()
        .remove_channels(args.prioritized_channels(), &args.feature_name())?;

    // Try to update the lock-file without the removed channels
    get_update_lock_file_and_prefix(
        &workspace.workspace().default_environment(),
        args.revalidate_config.update_mode(),
        UpdateLockFileOptions {
            lock_file_usage: LockFileUsage::Update,
            no_install: args.no_install_config.no_install,
            max_concurrent_solves: workspace.workspace().config().max_concurrent_solves(),
        },
        ReinstallPackages::default(),
        &[],
    )
    .await?;
    let workspace = workspace.save().await.into_diagnostic()?;

    // Report back to the user
    args.report("Removed", &workspace.channel_config())?;

    Ok(())
}
