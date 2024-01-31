use crate::environment::{get_up_to_date_prefix, LockFileUsage};

use crate::Project;
use clap::Parser;
use indexmap::IndexMap;
use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::Platform;
use std::str::FromStr;

#[derive(Parser, Debug, Default)]
pub struct Args {
    /// The platform name(s) to remove.
    #[clap(required = true, num_args=1..)]
    pub platform: Vec<String>,

    /// Don't update the environment, only remove the platform(s) from the lock-file.
    #[clap(long)]
    pub no_install: bool,
}

pub async fn execute(mut project: Project, args: Args) -> miette::Result<()> {
    // Determine which platforms to remove
    let platforms = args
        .platform
        .into_iter()
        .map(|platform_str| Platform::from_str(&platform_str))
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()?;

    let platforms_to_remove = platforms
        .into_iter()
        .filter(|x| project.platforms().contains(x))
        .collect_vec();

    if platforms_to_remove.is_empty() {
        eprintln!(
            "{}The platforms(s) are not present.",
            console::style(console::Emoji("✔ ", "")).green(),
        );
        return Ok(());
    }

    // Remove the platform(s) from the manifest
    project
        .manifest
        .remove_platforms(platforms_to_remove.iter().map(|p| p.to_string()))?;

    get_up_to_date_prefix(
        &project.default_environment(),
        LockFileUsage::Update,
        args.no_install,
        IndexMap::default(),
    )
    .await?;
    project.save()?;

    // Report back to the user
    for platform in platforms_to_remove {
        eprintln!(
            "{}Removed {}",
            console::style(console::Emoji("✔ ", "")).green(),
            platform,
        );
    }

    Ok(())
}
