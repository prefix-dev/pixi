use std::str::FromStr;

use chrono::Utc;
use clap::Parser;
use pixi_config::{Config, ConfigCli};
use pixi_global::Project;
use pixi_manifest::entry_as_written;
use rattler_conda_types::PackageName;

use crate::{
    global::sync,
    workspace::exclude_newer::{
        ExcludeNewerError, ExemptionCutoffArgs, add_exemptions, print_success, prune_exemptions,
        remove_exemptions,
    },
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
    /// Remove the cutoff, packages uploaded after it are no longer excluded.
    ///
    /// The cutoff cannot be removed while it has exemptions.
    Unset(UpdateArgs),
    /// Exempt package releases from the cutoff.
    ///
    /// Exemptions are match specs, a bare package name exempts every release.
    /// An existing exemption of the same package is replaced, with a warning
    /// if that moves its cutoff later.
    ///
    /// An exemption with a version only exempts the releases that are already
    /// published, so e.g. a later build of an exempt version is not exempt.
    ///
    /// Example:
    /// `pixi global exclude-newer exempt openssl==3.2.1 conda-forge::ruff`
    /// `pixi global exclude-newer exempt openssl==3.2.1 --cutoff 2026-10-12`
    #[clap(verbatim_doc_comment)]
    Exempt(ExemptArgs),
    /// Remove the exemptions of packages.
    ///
    /// Example:
    /// `pixi global exclude-newer unexempt python ruff`
    #[clap(verbatim_doc_comment)]
    Unexempt(UnexemptArgs),
    /// Remove the exemptions that no longer exempt anything.
    ///
    /// An exemption with a cutoff is stale once the cutoff of every channel
    /// it applies to is at or past the cutoff of the exemption.
    ///
    /// Removing a stale exemption doesn't change the solve, so the
    /// environments are not synced.
    ///
    /// Example:
    /// `pixi global exclude-newer prune --dry-run`
    #[clap(verbatim_doc_comment)]
    Prune(PruneArgs),
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
    pub(crate) cutoff: ExemptionCutoffArgs,

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

#[derive(Parser, Debug)]
pub struct PruneArgs {
    /// Only show the stale exemptions, without removing them
    #[clap(long)]
    pub dry_run: bool,

    #[clap(flatten)]
    pub config: ConfigCli,
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
        Some(Command::Prune(args)) => return prune(args).await,
    };
    let config = update.config.clone();
    let no_sync = update.no_sync;
    let original = Project::discover_or_create()
        .await?
        .with_cli_config(Config::with_cli_config(&config));
    let mut project = original.clone();

    let message = match args.command.as_ref().expect("handled above") {
        Command::Get | Command::Prune(_) => unreachable!("handled above"),
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
            add_exemptions(
                &args.specs,
                &args.cutoff,
                &channel_config,
                |name, exemption| {
                    Ok(project
                        .manifest
                        .add_exclude_newer_exemption(name, exemption)?)
                },
            )?
        }
        Command::Unexempt(args) => {
            remove_exemptions(&args.packages, "exclude-newer", |package| {
                let name = PackageName::from_str(package).map_err(ExcludeNewerError::from)?;
                Ok::<_, miette::Report>(project.manifest.remove_exclude_newer_exemption(&name)?)
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

    print_success(&message);
    Ok(())
}

/// Removes the stale exemptions of `exclude-newer` from the manifest.
async fn prune(args: &PruneArgs) -> miette::Result<()> {
    let mut project = Project::discover_or_create()
        .await?
        .with_cli_config(Config::with_cli_config(&args.config));
    let stale = project.stale_exclude_newer_exemptions(Utc::now())?;
    let Some(message) = prune_exemptions(&stale, args.dry_run, |name| {
        Ok(project.manifest.remove_exclude_newer_exemption(name)?)
    })?
    else {
        return Ok(());
    };
    project.manifest.save().await?;
    print_success(&message);
    Ok(())
}
