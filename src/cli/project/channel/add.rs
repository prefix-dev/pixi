use crate::{
    environment::{get_up_to_date_prefix, LockFileUsage},
    Project,
};

use super::AddRemoveArgs;

pub async fn execute(mut project: Project, args: AddRemoveArgs) -> miette::Result<()> {
    // Add the channels to the manifest
    project
        .manifest
        .add_channels(args.prioritized_channels(), &args.feature_name())?;

    // TODO: Update all environments touched by the features defined.
    get_up_to_date_prefix(
        &project.default_environment(),
        LockFileUsage::Update,
        args.no_install,
    )
    .await?;
    project.save()?;

    // Report back to the user
    args.report("Added", &project.channel_config());

    Ok(())
}
