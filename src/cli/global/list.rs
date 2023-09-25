use std::collections::HashSet;
use std::fmt::Display;
use std::str::FromStr;

use clap::Parser;
use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::PackageName;

use crate::cli::global::install::{
    bin_env_dir, find_and_map_executable_scripts, find_designated_package, BinDir, BinEnvDir,
    BinScriptMapping,
};
use crate::prefix::Prefix;

/// Lists all packages previously installed into a globally accessible location via `pixi global install`.
#[derive(Parser, Debug)]
pub struct Args {}

#[derive(Debug)]
struct InstalledPackageInfo {
    name: PackageName,
    binaries: Vec<String>,
}

impl Display for InstalledPackageInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let binaries = self
            .binaries
            .iter()
            .map(|name| format!("[bin] {}", console::style(name).bold()))
            .join("\n     -  ");
        write!(
            f,
            "  -  [package] {}\n     -  {binaries}",
            console::style(&self.name.as_source()).bold()
        )
    }
}

fn print_no_packages_found_message() {
    eprintln!(
        "{} No globally installed binaries found",
        console::style("!").yellow().bold()
    )
}

pub async fn execute(_args: Args) -> miette::Result<()> {
    let mut packages = vec![];
    let mut dir_contents = tokio::fs::read_dir(bin_env_dir()?)
        .await
        .into_diagnostic()?;
    while let Some(entry) = dir_contents.next_entry().await.into_diagnostic()? {
        if entry.file_type().await.into_diagnostic()?.is_dir() {
            let Ok(name) = PackageName::from_str(entry.file_name().to_string_lossy().as_ref()) else { continue };
            packages.push(name);
        }
    }

    let mut package_info = vec![];

    for package_name in packages {
        let Ok(BinEnvDir(bin_env_prefix)) = BinEnvDir::from_existing(&package_name).await else {
            print_no_packages_found_message();
            return Ok(());
        };
        let prefix = Prefix::new(bin_env_prefix)?;

        let Ok(bin_prefix) = BinDir::from_existing().await else {
            print_no_packages_found_message();
            return Ok(());
        };

        // Find the installed package in the environment
        let prefix_package = find_designated_package(&prefix, &package_name).await?;

        let binaries: Vec<_> =
            find_and_map_executable_scripts(&prefix, &prefix_package, &bin_prefix)
                .await?
                .into_iter()
                .map(
                    |BinScriptMapping {
                         global_binary_path: path,
                         ..
                     }| {
                        path.strip_prefix(&bin_prefix.0)
                            .expect("script paths were constructed by joining onto BinDir")
                            .to_string_lossy()
                            .to_string()
                    },
                )
                // Collecting to a HashSet first is a workaround for issue #317 and can be removed
                // once that is fixed.
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();

        package_info.push(InstalledPackageInfo {
            name: package_name,
            binaries,
        });
    }

    if package_info.is_empty() {
        print_no_packages_found_message();
    } else {
        eprintln!(
            "Globally installed binary packages:\n{}",
            package_info
                .into_iter()
                .map(|pkg| pkg.to_string())
                .join("\n")
        );
    }

    Ok(())
}
