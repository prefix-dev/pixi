use std::collections::HashSet;
use std::str::FromStr;

use clap::Parser;
use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::PackageName;

use crate::{global::install::ScriptExecMapping, prefix::Prefix};
use pixi_config::home_path;

use crate::global::{bin_env_dir, find_designated_package, BinDir, BinEnvDir};

/// Lists all packages previously installed into a globally accessible location via `pixi global install`.
#[derive(Parser, Debug)]
pub struct Args {}

#[derive(Debug)]
struct InstalledPackageInfo {
    /// The name of the installed package
    name: PackageName,

    /// The binaries installed by this package
    binaries: Vec<String>,

    /// The version of the installed package
    version: String,
}

fn print_no_packages_found_message() {
    eprintln!(
        "{} No globally installed binaries found",
        console::style("!").yellow().bold()
    )
}

pub async fn execute(_args: Args) -> miette::Result<()> {
    todo!()
}

/// List all globally installed packages
///
/// # Returns
///
/// A list of all globally installed packages represented as [`PackageName`]s
pub(super) async fn list_global_packages() -> miette::Result<Vec<PackageName>> {
    let mut packages = vec![];
    let bin_env_dir =
        bin_env_dir().ok_or(miette::miette!("Could not determine global envs directory"))?;
    let Ok(mut dir_contents) = tokio::fs::read_dir(bin_env_dir).await else {
        return Ok(vec![]);
    };

    while let Some(entry) = dir_contents.next_entry().await.into_diagnostic()? {
        if entry.file_type().await.into_diagnostic()?.is_dir() {
            if let Ok(name) = PackageName::from_str(entry.file_name().to_string_lossy().as_ref()) {
                packages.push(name);
            }
        }
    }

    packages.sort();
    Ok(packages)
}
