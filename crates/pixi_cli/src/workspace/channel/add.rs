use miette::IntoDiagnostic;
use pixi_core::{
    UpdateLockFileOptions, WorkspaceLocator,
    environment::{InstallFilter, get_update_lock_file_and_prefix},
    lock_file::{ReinstallPackages, UpdateMode},
};

use super::AddRemoveArgs;

pub async fn execute(args: AddRemoveArgs) -> miette::Result<()> {
    let mut workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?
        .with_cli_config(args.config.clone())
        .modify()?;

    // Add the channels to the manifest
    workspace.manifest().add_channels(
        args.prioritized_channels(),
        &args.feature_name(),
        args.prepend,
    )?;

    // TODO: Update all environments touched by the features defined.
    get_update_lock_file_and_prefix(
        &workspace.workspace().default_environment(),
        UpdateMode::Revalidate,
        UpdateLockFileOptions {
            lock_file_usage: args.lock_file_update_config.lock_file_usage()?,
            no_install: args.no_install_config.no_install,
            max_concurrent_solves: workspace.workspace().config().max_concurrent_solves(),
        },
        ReinstallPackages::default(),
        &InstallFilter::default(),
    )
    .await?;

    let workspace = workspace.save().await.into_diagnostic()?;

    // Report back to the user
    args.report("Added", &workspace.channel_config())?;

    Ok(())
}
