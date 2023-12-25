use std::str::FromStr;

use crate::environment::update_prefix;
use crate::lock_file::{load_lock_file, update_lock_file_conda};
use crate::prefix::Prefix;
use crate::Project;
use clap::Parser;
use itertools::Itertools;
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
}

pub async fn execute(mut project: Project, args: Args) -> miette::Result<()> {
    // Determine which platforms are missing
    let platforms = args
        .platform
        .into_iter()
        .map(|platform_str| Platform::from_str(&platform_str))
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()?;

    let missing_platforms = platforms
        .into_iter()
        .filter(|x| !project.platforms().contains(x))
        .collect_vec();

    if missing_platforms.is_empty() {
        eprintln!(
            "{}All platform(s) have already been added.",
            console::style(console::Emoji("✔ ", "")).green(),
        );
        return Ok(());
    }

    // Load the existing lock-file
    let lock_file = load_lock_file(&project).await?;

    // Add the platforms to the lock-file
    project.manifest.add_platforms(missing_platforms.iter())?;

    // Try to update the lock-file with the new channels
    let lock_file = update_lock_file_conda(&project, lock_file, None).await?;
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
    for platform in missing_platforms {
        eprintln!(
            "{}Added {}",
            console::style(console::Emoji("✔ ", "")).green(),
            platform
        );
    }

    Ok(())
}
