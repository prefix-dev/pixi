use std::str::FromStr;

use clap::Parser;
use pixi_config::{Config, ConfigCli};
use pixi_global::Project;
use rattler_conda_types::PackageName;

use crate::{global::sync, workspace::exclude_newer::conda_toml_value};

/// Manage the `exclude-newer` configuration of the global manifest.
///
/// Packages uploaded after the cutoff are excluded from the solve of every
/// global environment, except for the exempted releases. The environments are
/// synced after every change.
#[derive(Parser, Debug)]
pub struct Args {
    /// The subcommand to execute, shows the configuration when omitted
    #[clap(subcommand)]
    pub command: Option<Command>,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Show the `exclude-newer` configuration.
    Get,
    /// Set the cutoff, packages uploaded after it are excluded from the solve.
    ///
    /// Example:
    /// `pixi global exclude-newer set 7d`
    #[clap(verbatim_doc_comment)]
    Set(SetArgs),
    /// Remove the cutoff.
    ///
    /// The cutoff cannot be removed while it has exemptions.
    Unset(ConfigArgs),
    /// Exempt package releases from the cutoff.
    ///
    /// Exemptions are match specs, a bare package name exempts every release.
    /// An existing exemption of the same package is replaced.
    ///
    /// Example:
    /// `pixi global exclude-newer exempt 'python=3.13.*' conda-forge::ruff`
    #[clap(verbatim_doc_comment)]
    Exempt(ExemptArgs),
    /// Remove the exemptions of packages.
    ///
    /// Example:
    /// `pixi global exclude-newer unexempt python ruff`
    #[clap(verbatim_doc_comment)]
    Unexempt(UnexemptArgs),
}

#[derive(Parser, Debug)]
pub struct ConfigArgs {
    #[clap(flatten)]
    pub config: ConfigCli,
}

#[derive(Parser, Debug)]
pub struct SetArgs {
    /// The cutoff: an RFC 3339 timestamp, a date like `2026-01-01`, or a
    /// duration relative to the time of the solve like `7d`
    pub cutoff: String,

    #[clap(flatten)]
    pub config: ConfigCli,
}

#[derive(Parser, Debug)]
pub struct ExemptArgs {
    /// The releases to exempt, e.g. `python`, `python=3.13.*` or
    /// `conda-forge::ruff`
    #[arg(required = true, num_args = 1.., value_name = "SPEC")]
    pub specs: Vec<String>,

    #[clap(flatten)]
    pub config: ConfigCli,
}

#[derive(Parser, Debug)]
pub struct UnexemptArgs {
    /// The packages whose exemptions are removed
    #[arg(required = true, num_args = 1.., value_name = "PACKAGE")]
    pub packages: Vec<String>,

    #[clap(flatten)]
    pub config: ConfigCli,
}

#[derive(Debug, thiserror::Error, miette::Diagnostic)]
enum ExcludeNewerError {
    #[error(transparent)]
    InvalidPackageName(#[from] rattler_conda_types::InvalidPackageNameError),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = match &args.command {
        None | Some(Command::Get) => {
            let project = Project::discover_or_create().await?;
            let exclude_newer = &project.manifest.parsed.exclude_newer;
            if exclude_newer.is_empty() {
                eprintln!("No exclude-newer configured.");
            } else {
                println!("exclude-newer = {}", conda_toml_value(exclude_newer));
            }
            return Ok(());
        }
        Some(Command::Set(SetArgs { config, .. }))
        | Some(Command::Unset(ConfigArgs { config }))
        | Some(Command::Exempt(ExemptArgs { config, .. }))
        | Some(Command::Unexempt(UnexemptArgs { config, .. })) => config.clone(),
    };
    let mut project = Project::discover_or_create()
        .await?
        .with_cli_config(Config::with_cli_config(&config));

    let message = match args.command.expect("handled above") {
        Command::Get => unreachable!("handled above"),
        Command::Set(args) => {
            project
                .manifest
                .set_exclude_newer_cutoff(Some(&args.cutoff))?;
            format!("Set the exclude-newer cutoff to '{}'.", args.cutoff)
        }
        Command::Unset(_) => {
            project.manifest.set_exclude_newer_cutoff(None)?;
            "Removed the exclude-newer cutoff.".to_string()
        }
        Command::Exempt(args) => {
            let channel_config = project.config().global_channel_config().clone();
            for spec in &args.specs {
                let (name, spec) = pixi_manifest::parse_conda_exemption(spec, &channel_config)?;
                project.manifest.add_exclude_newer_exemption(name, spec)?;
            }
            format!("Exempted {} from exclude-newer.", args.specs.join(", "))
        }
        Command::Unexempt(args) => {
            let mut missing = Vec::new();
            for package in &args.packages {
                let name = PackageName::from_str(package).map_err(ExcludeNewerError::from)?;
                if !project.manifest.remove_exclude_newer_exemption(&name)? {
                    missing.push(package.as_str());
                }
            }
            if !missing.is_empty() {
                miette::bail!(
                    "{} {} not exempt from exclude-newer",
                    missing.join(", "),
                    if missing.len() == 1 { "is" } else { "are" }
                );
            }
            format!(
                "Removed the exclude-newer exemptions of {}.",
                args.packages.join(", ")
            )
        }
    };

    project.manifest.save().await?;
    eprintln!(
        "{}{message}",
        console::style(console::Emoji("✔ ", "")).green()
    );

    // A stricter cutoff can take the environments out of sync.
    sync::execute(sync::Args { config }).await
}
