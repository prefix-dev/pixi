use std::collections::HashSet;
use std::str::FromStr;

use clap::Parser;
use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::PackageName;

use crate::cli::global::install::{
    bin_env_dir, find_and_map_executable_scripts, find_designated_package, home_path, BinDir,
    BinEnvDir, BinScriptMapping,
};
use crate::prefix::Prefix;

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
    let packages = list_global_packages().await?;

    let mut package_info = vec![];

    for package_name in packages {
        let Ok(BinEnvDir(bin_env_prefix)) = BinEnvDir::from_existing(&package_name).await else {
            print_no_packages_found_message();
            return Ok(());
        };
        let prefix = Prefix::new(bin_env_prefix);

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

        let version = prefix_package
            .repodata_record
            .package_record
            .version
            .to_string();
        package_info.push(InstalledPackageInfo {
            name: package_name,
            binaries,
            version,
        });
    }

    if package_info.is_empty() {
        print_no_packages_found_message();
    } else {
        let path = home_path()?;
        let len = package_info.len();
        let mut message = String::new();
        for (idx, pkgi) in package_info.into_iter().enumerate() {
            let last = (idx + 1) == len;
            let no_binary = pkgi.binaries.is_empty();

            if last {
                message.push_str("└──");
            } else {
                message.push_str("├──");
            }

            message.push_str(&format!(
                " {} {}",
                console::style(&pkgi.name.as_source()).bold().magenta(),
                console::style(&pkgi.version).bright().black()
            ));

            if !no_binary {
                let p = if last { " " } else { "|" };
                message.push_str(&format!(
                    "\n{}   └─ exec: {}",
                    p,
                    pkgi.binaries
                        .iter()
                        .map(|x| console::style(x).green())
                        .join(", ")
                ));
            }

            if !last {
                message.push('\n');
            }
        }

        eprintln!("Global install location: {}\n{}", path.display(), message);
    }

    Ok(())
}

pub(super) async fn list_global_packages() -> Result<Vec<PackageName>, miette::ErrReport> {
    let mut packages = vec![];
    let Ok(mut dir_contents) = tokio::fs::read_dir(bin_env_dir()?)
        .await else { return Ok(vec![]) };

    while let Some(entry) = dir_contents.next_entry().await.into_diagnostic()? {
        if entry.file_type().await.into_diagnostic()?.is_dir() {
            let Ok(name) = PackageName::from_str(entry.file_name().to_string_lossy().as_ref())
            else {
                continue;
            };
            packages.push(name);
        }
    }

    Ok(packages)
}
