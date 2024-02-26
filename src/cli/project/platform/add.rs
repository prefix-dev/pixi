use std::str::FromStr;

use crate::environment::{get_up_to_date_prefix, LockFileUsage};
use crate::{FeatureName, Project};
use clap::Parser;
use indexmap::IndexMap;
use miette::IntoDiagnostic;
use rattler_conda_types::Platform;

#[derive(Parser, Debug, Default)]
pub struct Args {
    /// The platform name(s) to add.
    #[clap(required = true, num_args=1..)]
    pub platform: Vec<String>,

    /// Don't update the environment, only add changed packages to the lock-file.
    #[clap(long)]
    pub no_install: bool,

    /// The name of the feature to add the platform to.
    #[clap(long, short)]
    pub feature: Option<String>,
}

pub async fn execute(mut project: Project, args: Args) -> miette::Result<()> {
    let feature_name = args
        .feature
        .map_or(FeatureName::Default, FeatureName::Named);

    // Determine which platforms are missing
    let platforms = args
        .platform
        .into_iter()
        .map(|platform_str| Platform::from_str(&platform_str))
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()?;

    // Add the platforms to the lock-file
    project
        .manifest
        .add_platforms(platforms.iter(), &feature_name)?;

    // Try to update the lock-file with the new channels
    get_up_to_date_prefix(
        &project.default_environment(),
        LockFileUsage::Update,
        args.no_install,
        IndexMap::default(),
    )
    .await?;
    project.save()?;

    // Report back to the user
    for platform in platforms {
        eprintln!(
            "{}Added {}",
            console::style(console::Emoji("✔ ", "")).green(),
            match &feature_name {
                FeatureName::Default => platform.to_string(),
                FeatureName::Named(name) => format!("{} to the feature {}", platform, name),
            }
        );
    }

    Ok(())
}
