use crate::environment::update_prefix;
use crate::lock_file::{load_lock_file, update_lock_file};
use crate::prefix::Prefix;
use crate::Project;
use clap::Parser;
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

    // Load the existing lock-file
    let lock_file = load_lock_file(&project).await?;

    // Remove the platform(s) from the manifest
    project
        .manifest
        .remove_platforms(platforms_to_remove.iter().map(|p| p.to_string()))?;

    // Try to update the lock-file without the removed platform(s)
    let lock_file = update_lock_file(&project, lock_file, None).await?;
    project.save()?;

    // Update the installation if needed
    if !args.no_install {
        // Get the currently installed packages
        let prefix = Prefix::new(project.environment_dir())?;
        let installed_packages = prefix.find_installed_packages(None).await?;

        // Update the prefix
        update_prefix(
            project.pypi_package_db()?,
            &prefix,
            installed_packages,
            &lock_file,
            Platform::current(),
        )
        .await?;
    }

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
