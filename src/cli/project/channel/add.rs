use crate::{
    environment::{get_update_lock_file_and_prefix, LockFileUsage},
    lock_file::UpdateMode,
    Project,
};

use super::AddRemoveArgs;

pub async fn execute(mut project: Project, args: AddRemoveArgs) -> miette::Result<()> {
    // Add the channels to the manifest
    project.manifest.add_channels(
        args.prioritized_channels(),
        &args.feature_name(),
        args.prepend,
    )?;

    // TODO: Update all environments touched by the features defined.
    get_update_lock_file_and_prefix(
        &project.default_environment(),
        LockFileUsage::Update,
        args.no_install,
        UpdateMode::Revalidate,
    )
    .await?;
    project.save()?;

    // Report back to the user
    args.report("Added", &project.channel_config())?;

    Ok(())
}
