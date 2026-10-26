use std::str::FromStr;

use clap::Parser;
use pixi_config::{Config, ConfigCli};
use pixi_global::Project;
use pixi_manifest::entry_as_written;
use rattler_conda_types::PackageName;

use crate::{
    global::sync,
    workspace::exclude_newer::{ExcludeNewerError, remove_exemptions},
};

/// Manage the `exclude-newer` configuration of the global manifest.
///
/// Packages uploaded after the cutoff are excluded from the solve of every
/// global environment, except for the exempted releases. The environments are
/// synced after every change, which is reverted if the sync fails.
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
    Unset(UpdateArgs),
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
pub struct UpdateArgs {
    /// Only edit the manifest, without syncing the environments
    #[clap(long)]
    pub no_sync: bool,

    #[clap(flatten)]
    pub config: ConfigCli,
}

#[derive(Parser, Debug)]
pub struct SetArgs {
    /// The cutoff: an RFC 3339 timestamp, a date like `2026-01-01`, or a
    /// duration relative to the time of the solve like `7d`
    pub cutoff: String,

    #[clap(flatten)]
    pub update: UpdateArgs,
}

#[derive(Parser, Debug)]
pub struct ExemptArgs {
    /// The releases to exempt, e.g. `python`, `python=3.13.*` or
    /// `conda-forge::ruff`
    #[arg(required = true, num_args = 1.., value_name = "SPEC")]
    pub specs: Vec<String>,

    #[clap(flatten)]
    pub update: UpdateArgs,
}

#[derive(Parser, Debug)]
pub struct UnexemptArgs {
    /// The packages whose exemptions are removed
    #[arg(required = true, num_args = 1.., value_name = "PACKAGE")]
    pub packages: Vec<String>,

    #[clap(flatten)]
    pub update: UpdateArgs,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let update = match &args.command {
        None | Some(Command::Get) => {
            let project = Project::discover_or_create().await?;
            match project
                .manifest
                .document
                .as_table()
                .get("exclude-newer")
                .and_then(entry_as_written)
            {
                Some(value) => println!("exclude-newer = {value}"),
                None => eprintln!("No exclude-newer configured."),
            }
            return Ok(());
        }
        Some(Command::Set(SetArgs { update, .. }))
        | Some(Command::Unset(update))
        | Some(Command::Exempt(ExemptArgs { update, .. }))
        | Some(Command::Unexempt(UnexemptArgs { update, .. })) => update,
    };
    let config = update.config.clone();
    let no_sync = update.no_sync;
    let original = Project::discover_or_create()
        .await?
        .with_cli_config(Config::with_cli_config(&config));
    let mut project = original.clone();

    let message = match args.command.as_ref().expect("handled above") {
        Command::Get => unreachable!("handled above"),
        Command::Set(args) => {
            project
                .manifest
                .set_exclude_newer_cutoff(Some(args.cutoff.trim()))?;
            format!("Set the exclude-newer cutoff to '{}'.", args.cutoff.trim())
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
            remove_exemptions(&args.packages, "exclude-newer", |package| {
                let name = PackageName::from_str(package).map_err(ExcludeNewerError::from)?;
                project.manifest.remove_exclude_newer_exemption(&name)
            })?;
            format!(
                "Removed the exclude-newer exemptions of {}.",
                args.packages.join(", ")
            )
        }
    };

    if project.manifest.document.to_string() == original.manifest.document.to_string() {
        eprintln!("The exclude-newer configuration is unchanged.");
        return Ok(());
    }

    project.manifest.save().await?;

    // A stricter cutoff can take the environments out of sync. If they can't
    // be synced with the new configuration, restore the original manifest and
    // sync back to it.
    if !no_sync
        && let Err(err) = sync::execute(sync::Args {
            config: config.clone(),
        })
        .await
    {
        original.manifest.save().await?;
        if let Err(revert_err) = sync::execute(sync::Args { config }).await {
            tracing::warn!("Reverting of the operation failed");
            tracing::info!("Reversion error: {:?}", revert_err);
        }
        return Err(err.wrap_err("Couldn't update the exclude-newer configuration"));
    }

    eprintln!(
        "{}{message}",
        console::style(console::Emoji("✔ ", "")).green()
    );
    Ok(())
}
