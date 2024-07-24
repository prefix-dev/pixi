use std::collections::HashSet;

use clap::Parser;
use clap_verbosity_flag::{Level, Verbosity};
use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::PackageName;

use crate::cli::has_specs::HasSpecs;
use crate::prefix::Prefix;

use super::common::{find_designated_package, BinDir, BinEnvDir};
use super::install::{find_and_map_executable_scripts, BinScriptMapping, BinarySelector};

/// Removes a package previously installed into a globally accessible location via `pixi global install`.
#[derive(Parser, Debug)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    /// Specifies the packages that are to be removed.
    #[arg(num_args = 1..)]
    packages: Vec<String>,

    #[command(flatten)]
    verbose: Verbosity,
}

impl HasSpecs for Args {
    fn packages(&self) -> Vec<&str> {
        self.packages.iter().map(AsRef::as_ref).collect()
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    for (package_name, _) in args.specs()? {
        remove_global_package(package_name, &args.verbose).await?;
    }

    Ok(())
}

async fn remove_global_package(
    package_name: PackageName,
    verbose: &Verbosity,
) -> miette::Result<()> {
    let BinEnvDir(bin_prefix) = BinEnvDir::from_existing(&package_name).await?;
    let prefix = Prefix::new(bin_prefix.clone());

    // Find the installed package in the environment
    let prefix_package = find_designated_package(&prefix, &package_name).await?;

    // Construct the paths to all the installed package executables, which are what we need to remove.
    // TODO -- figure out if we need to Binary Selector All works well here
    let paths_to_remove: Vec<_> = find_and_map_executable_scripts(
        &prefix,
        &prefix_package,
        &BinDir::from_existing().await?,
        &BinarySelector::All,
    )
    .await?
    .into_iter()
    .map(
        |BinScriptMapping {
             global_binary_path: path,
             ..
         }| path,
    )
    // Collecting to a HashSet first is a workaround for issue #317 and can be removed
    // once that is fixed.
    .collect::<HashSet<_>>()
    .into_iter()
    .collect();

    let dirs_to_remove: Vec<_> = vec![bin_prefix];

    if verbose.log_level().unwrap_or(Level::Error) >= Level::Warn {
        let whitespace = console::Emoji("  ", "").to_string();
        let names_to_remove = dirs_to_remove
            .iter()
            .map(|dir| dir.to_string_lossy())
            .chain(paths_to_remove.iter().map(|path| path.to_string_lossy()))
            .join(&format!("\n{whitespace} -  "));

        eprintln!(
            "{} Removing the following files and directories:\n{whitespace} -  {names_to_remove}",
            console::style("!").yellow().bold(),
        )
    }

    let mut errors = vec![];

    for file in paths_to_remove {
        if let Err(e) = tokio::fs::remove_file(&file).await.into_diagnostic() {
            errors.push((file, e))
        }
    }

    for dir in dirs_to_remove {
        if let Err(e) = tokio::fs::remove_dir_all(&dir).await.into_diagnostic() {
            errors.push((dir, e))
        }
    }

    if errors.is_empty() {
        eprintln!(
            "{}Successfully removed global package {}",
            console::style(console::Emoji("✔ ", "")).green(),
            console::style(package_name.as_source()).bold(),
        );
    } else {
        let whitespace = console::Emoji("  ", "").to_string();
        let error_string = errors
            .into_iter()
            .map(|(file, e)| format!("{} (on {})", e, file.to_string_lossy()))
            .join(&format!("\n{whitespace} -  "));
        miette::bail!(
            "got multiple errors trying to remove global package {}:\n{} -  {}",
            package_name.as_source(),
            whitespace,
            error_string,
        );
    }

    Ok(())
}
