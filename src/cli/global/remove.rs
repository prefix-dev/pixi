use std::collections::HashSet;
use std::str::FromStr;

use clap::Parser;
use clap_verbosity_flag::{Level, Verbosity};
use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::MatchSpec;
use rattler_shell::shell::ShellEnum;

use crate::cli::global::install::{
    create_activation_script, create_executable_scripts, find_designated_package, BinEnvDir,
};
use crate::prefix::Prefix;

/// Removes a package previously installed into a globally accessible location via `pixi global install`.
#[derive(Parser, Debug)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    /// Specifies the package that is to be removed.
    package: String,
    #[command(flatten)]
    verbose: Verbosity,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    // Find the MatchSpec we want to install
    let package_matchspec = MatchSpec::from_str(&args.package).into_diagnostic()?;
    let package_name = package_matchspec.name.clone().ok_or_else(|| {
        miette::miette!(
            "could not find package name in MatchSpec {}",
            package_matchspec
        )
    })?;
    let BinEnvDir(bin_prefix) = BinEnvDir::from_existing(&package_name).await?;
    let prefix = Prefix::new(bin_prefix.clone())?;

    // Find the installed package in the environment
    let prefix_package = find_designated_package(&prefix, &package_name).await?;

    // Determine the shell to use for the invocation script
    let shell: ShellEnum = if cfg!(windows) {
        rattler_shell::shell::CmdExe.into()
    } else {
        rattler_shell::shell::Bash.into()
    };

    // Construct the reusable activation script for the shell and generate an invocation script
    // for each executable added by the package to the environment.
    let activation_script = create_activation_script(&prefix, shell.clone())?;
    let paths_to_remove: Vec<_> =
        create_executable_scripts(&prefix, &prefix_package, &shell, activation_script, true)
            .await?
            // Collecting to a HashSet first is a workaround for issue #317 and can be removed
            // once that is fixed.
            .into_iter()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

    let dirs_to_remove: Vec<_> = vec![bin_prefix];

    if args.verbose.log_level().unwrap_or(Level::Error) >= Level::Warn {
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
            console::style(package_name).bold(),
        );
    } else {
        let whitespace = console::Emoji("  ", "").to_string();
        let error_string = errors
            .into_iter()
            .map(|(file, e)| format!("{} (on {})", e, file.to_string_lossy()))
            .join(&format!("\n{whitespace} -  "));
        miette::bail!(
            "got multiple errors trying to remove global package {}:\n{} -  {}",
            package_name,
            whitespace,
            error_string,
        );
    }

    Ok(())
}
