use std::path::PathBuf;

use clap::Parser;
use rattler_conda_types::Platform;

use crate::Project;

/// Remove a dependency from the project
#[derive(Debug, Parser)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    /// Name of package to remove
    #[arg(required = true)]
    pub packages: Vec<String>,

    /// Platform to remove the dependency - applicable only for platform specific dependencies
    #[arg(long, short)]
    pub platform: Option<Platform>,

    /// The path to 'pixi.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let mut project = Project::load_or_else_discover(args.manifest_path.as_deref())?;
    let packages = args.packages;
    let platform = args.platform;

    for package in packages {
        match project.remove_dependency(package.as_str(), platform) {
            Ok(_) => continue,
            Err(_) => {
                eprintln!(
                    "{}Could not remove '{}'",
                    console::style(console::Emoji("❌ ", "X")).red(),
                    console::style(&package).bold(),
                );
            }
        }
    }

    Ok(())
}
