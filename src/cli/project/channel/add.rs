use crate::{
    environment::{get_update_lock_file_and_prefix, LockFileUsage},
    lock_file::UpdateMode,
    Project, UpdateLockFileOptions,
};

use super::AddRemoveArgs;

pub async fn execute(args: AddRemoveArgs) -> miette::Result<()> {
    let mut project = Project::load_or_else_discover(args.project_config.manifest_path.as_deref())?
        .with_cli_config(args.clone().prefix_update_config.config);

    // Add the channels to the manifest
    project.manifest.add_channels(
        args.prioritized_channels(),
        &args.feature_name(),
        args.prepend,
    )?;

    // TODO: Update all environments touched by the features defined.
    get_update_lock_file_and_prefix(
        &project.default_environment(),
        UpdateMode::Revalidate,
        UpdateLockFileOptions {
            lock_file_usage: LockFileUsage::Update,
            no_install: args.prefix_update_config.no_install(),
            max_concurrent_solves: args.prefix_update_config.config.max_concurrent_solves,
        },
    )
    .await?;
    project.save()?;

    // Report back to the user
    args.report("Added", &project.channel_config())?;

    Ok(())
}
