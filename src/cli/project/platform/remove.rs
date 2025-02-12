use clap::Parser;
use miette::IntoDiagnostic;
use pixi_manifest::FeatureName;
use rattler_conda_types::Platform;

use crate::{
    environment::{get_update_lock_file_and_prefix, LockFileUsage},
    lock_file::UpdateMode,
    UpdateLockFileOptions, Workspace,
};

#[derive(Parser, Debug, Default)]
pub struct Args {
    /// The platform name(s) to remove.
    #[clap(required = true, num_args=1..)]
    pub platforms: Vec<Platform>,

    /// Don't update the environment, only remove the platform(s) from the
    /// lock-file.
    #[clap(long)]
    pub no_install: bool,

    /// The name of the feature to remove the platform from.
    #[clap(long, short)]
    pub feature: Option<String>,
}

pub async fn execute(workspace: Workspace, args: Args) -> miette::Result<()> {
    let feature_name = args
        .feature
        .map_or(FeatureName::Default, FeatureName::Named);

    let mut workspace = workspace.modify()?;

    // Remove the platform(s) from the manifest
    workspace
        .manifest()
        .remove_platforms(args.platforms.clone(), &feature_name)?;

    get_update_lock_file_and_prefix(
        &workspace.workspace().default_environment(),
        UpdateMode::Revalidate,
        UpdateLockFileOptions {
            lock_file_usage: LockFileUsage::Update,
            no_install: args.no_install,
            max_concurrent_solves: workspace.workspace().config().max_concurrent_solves(),
        },
    )
    .await?;
    workspace.save().await.into_diagnostic()?;

    // Report back to the user
    for platform in args.platforms {
        eprintln!(
            "{}Removed {}",
            console::style(console::Emoji("✔ ", "")).green(),
            match &feature_name {
                FeatureName::Default => platform.to_string(),
                FeatureName::Named(name) => format!("{} from the feature {}", platform, name),
            },
        );
    }

    Ok(())
}
