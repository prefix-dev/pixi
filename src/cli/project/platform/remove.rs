use std::str::FromStr;

use crate::lock_file::UpdateMode;
use crate::{
    environment::{get_update_lock_file_and_prefix, LockFileUsage},
    Project, UpdateLockFileOptions,
};
use clap::Parser;
use miette::IntoDiagnostic;
use pixi_manifest::FeatureName;
use rattler_conda_types::Platform;

#[derive(Parser, Debug, Default)]
pub struct Args {
    /// The platform name(s) to remove.
    #[clap(required = true, num_args=1..)]
    pub platform: Vec<String>,

    /// Don't update the environment, only remove the platform(s) from the
    /// lock-file.
    #[clap(long)]
    pub no_install: bool,

    /// The name of the feature to remove the platform from.
    #[clap(long, short)]
    pub feature: Option<String>,
}

pub async fn execute(mut project: Project, args: Args) -> miette::Result<()> {
    let feature_name = args
        .feature
        .map_or(FeatureName::Default, FeatureName::Named);

    // Determine which platforms to remove
    let platforms = args
        .platform
        .into_iter()
        .map(|platform_str| Platform::from_str(&platform_str))
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()?;

    // Remove the platform(s) from the manifest
    project
        .manifest
        .remove_platforms(platforms.clone(), &feature_name)?;

    get_update_lock_file_and_prefix(
        &project.default_environment(),
        UpdateMode::Revalidate,
        UpdateLockFileOptions {
            lock_file_usage: LockFileUsage::Update,
            no_install: args.no_install,
            max_concurrent_solves: project.config().max_concurrent_solves,
        },
    )
    .await?;
    project.save()?;

    // Report back to the user
    for platform in platforms {
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
