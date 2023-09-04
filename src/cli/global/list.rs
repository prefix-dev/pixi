use std::collections::HashSet;
use std::fmt::Display;

use clap::Parser;
use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_shell::shell::ShellEnum;

use crate::cli::global::install::{
    create_activation_script, create_executable_scripts, find_designated_package, BinEnvDir,
};
use crate::prefix::Prefix;

use super::install::{bin_env_dir, BinDir};

/// Lists all packages previously installed into a globally accessible location via `pixi global install`.
#[derive(Parser, Debug)]
pub struct Args {}

#[derive(Debug)]
struct InstalledPackageInfo {
    name: String,
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
            console::style(&self.name).bold()
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
            packages.push(entry.file_name().to_string_lossy().to_string());
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

        // Determine the shell to use for the invocation script
        let shell: ShellEnum = if cfg!(windows) {
            rattler_shell::shell::CmdExe.into()
        } else {
            rattler_shell::shell::Bash.into()
        };

        // Do a dry run creation of the executable scripts to figure out what installed paths we have.
        let activation_script = create_activation_script(&prefix, shell.clone())?;
        let binaries: Vec<_> =
            create_executable_scripts(&prefix, &prefix_package, &shell, activation_script, true)
                .await?
                .into_iter()
                .map(|path| {
                    path.strip_prefix(&bin_prefix.0)
                        .expect("script paths were constructed by joining onto BinDir")
                        .to_string_lossy()
                        .to_string()
                })
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
