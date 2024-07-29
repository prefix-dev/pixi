use crate::{
    environment::{get_up_to_date_prefix, LockFileUsage},
    Project,
};

use super::AddRemoveArgs;

pub async fn execute(mut project: Project, args: AddRemoveArgs) -> miette::Result<()> {
    // Remove the channels from the manifest
    project
        .manifest
        .remove_channels(args.prioritized_channels(), &args.feature_name())?;

    // Try to update the lock-file without the removed channels
    get_up_to_date_prefix(
        &project.default_environment(),
        LockFileUsage::Update,
        args.no_install,
    )
    .await?;
    project.save()?;

    // Report back to the user
    args.report("Removed", &project.channel_config());

    Ok(())
}
