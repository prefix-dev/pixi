use std::{path::Path, str::FromStr};

use chrono::{DateTime, SubsecRound, TimeDelta, Utc};
use clap::Parser;
use itertools::Itertools;
use pixi_config::ConfigCli;
use pixi_core::{
    InstallFilter, UpdateLockFileOptions, Workspace, WorkspaceLocator,
    environment::get_update_lock_file_and_prefix,
    lock_file::{ReinstallPackages, UpdateMode},
    workspace::WorkspaceMut,
};
use pixi_manifest::{
    ExcludeNewerExemption, FeaturesExt, ManifestDocument, ManifestKind, entry_as_written,
};
use pixi_pypi_spec::PypiPackageName;
use pixi_spec::{BinarySpec, ExcludeNewer};
use rattler_conda_types::{ChannelConfig, PackageName, VersionSpec};

use crate::cli_config::{LockFileUpdateConfig, NoInstallConfig, WorkspaceConfig};

/// Commands to manage the `exclude-newer` configuration of the workspace.
///
/// Packages uploaded after the cutoff are excluded from the solve, except for
/// the exempted releases.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,

    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    /// The subcommand to execute, shows the configuration when omitted
    #[clap(subcommand)]
    pub command: Option<Command>,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Show the `exclude-newer` and `pypi-exclude-newer` configuration.
    Get,
    /// Set the cutoff, packages uploaded after it are excluded from the solve.
    ///
    /// Example:
    /// `pixi workspace exclude-newer set 7d`
    #[clap(verbatim_doc_comment)]
    Set(SetArgs),
    /// Remove the cutoff, packages uploaded after it are no longer excluded.
    ///
    /// The cutoff of `exclude-newer` cannot be removed while it has exemptions.
    /// Without a `pypi-exclude-newer` cutoff, PyPI packages fall back to the
    /// cutoff of `exclude-newer`.
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
    /// `pixi workspace exclude-newer exempt openssl==3.2.1 conda-forge::ruff`
    /// `pixi workspace exclude-newer exempt openssl==3.2.1 --cutoff 2026-10-12`
    #[clap(verbatim_doc_comment)]
    Exempt(ExemptArgs),
    /// Remove the exemptions of packages.
    ///
    /// Example:
    /// `pixi workspace exclude-newer unexempt python ruff`
    #[clap(verbatim_doc_comment)]
    Unexempt(UnexemptArgs),
    /// Remove the exemptions that no longer exempt anything.
    ///
    /// An exemption with a cutoff is stale once the cutoff of every channel
    /// it applies to is at or past the cutoff of the exemption.
    ///
    /// Removing a stale exemption doesn't change the solve, so the lock file
    /// is not updated.
    ///
    /// Example:
    /// `pixi workspace exclude-newer prune --dry-run`
    #[clap(verbatim_doc_comment)]
    Prune(PruneArgs),
}

#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub(crate) enum ExcludeNewerError {
    #[error(transparent)]
    InvalidPackageName(#[from] rattler_conda_types::InvalidPackageNameError),
    #[error(transparent)]
    InvalidPypiPackageName(#[from] pep508_rs::InvalidNameError),
    #[error(
        "{} {} not exempt from {target}",
        .packages.join(", "),
        if .packages.len() == 1 { "is" } else { "are" }
    )]
    NotExempt {
        packages: Vec<String>,
        target: &'static str,
    },
    #[error("failed to save the manifest")]
    Save(#[from] std::io::Error),
    #[error("'{0}' is not a valid exemption cutoff")]
    #[diagnostic(help(
        "use `now`, an RFC 3339 timestamp, or a date like `2026-01-01`, durations are not supported"
    ))]
    InvalidExemptionCutoff(String),
    #[error("PyPI exemptions do not support a cutoff")]
    #[diagnostic(help("remove `--cutoff` and `--no-cutoff`"))]
    PypiExemptionCutoff,
}

/// The cutoff of new exemptions.
#[derive(Parser, Debug)]
pub(crate) struct ExemptionCutoffArgs {
    /// Only exempt the releases published up to this point in time: `now`,
    /// an RFC 3339 timestamp, or a date like `2026-01-01`, which includes the
    /// whole day in UTC. Defaults to `now` for specs with a version, e.g.
    /// `openssl==3.2.1`
    #[arg(long, value_name = "CUTOFF")]
    pub cutoff: Option<String>,

    /// Exempt every matching release, including the ones published in the
    /// future
    #[arg(long, conflicts_with = "cutoff")]
    pub no_cutoff: bool,
}

impl ExemptionCutoffArgs {
    /// Returns whether `--cutoff` or `--no-cutoff` was passed.
    fn is_set(&self) -> bool {
        self.cutoff.is_some() || self.no_cutoff
    }

    /// Returns the cutoff of an exemption for `spec`, resolving `now` to the
    /// current time.
    pub(crate) fn cutoff_for(
        &self,
        spec: &BinarySpec,
        now: DateTime<Utc>,
    ) -> Result<Option<DateTime<Utc>>, ExcludeNewerError> {
        if self.no_cutoff {
            return Ok(None);
        }
        match self.cutoff.as_deref() {
            Some("now") => Ok(Some(now)),
            Some(cutoff) => match ExcludeNewer::from_str(cutoff) {
                Ok(ExcludeNewer::Timestamp(cutoff)) => Ok(Some(cutoff)),
                Ok(ExcludeNewer::Duration(_)) | Err(_) => Err(
                    ExcludeNewerError::InvalidExemptionCutoff(cutoff.to_string()),
                ),
            },
            // A bare name or `*` trusts the package entirely, any other
            // version only the releases that were vetted.
            None => Ok(has_version(spec).then_some(now)),
        }
    }
}

fn has_version(spec: &BinarySpec) -> bool {
    let version = match spec {
        BinarySpec::Version(version) => Some(version),
        BinarySpec::DetailedVersion(detailed) => detailed.version.as_ref(),
        BinarySpec::Url(_) | BinarySpec::Path(_) => None,
    };
    version.is_some_and(|version| *version != VersionSpec::Any)
}

/// Returns the current time, rounded up to seconds so it reads well in the
/// manifest. Rounding down would leave out releases published earlier in the
/// same second.
fn now() -> DateTime<Utc> {
    let now = Utc::now();
    let seconds = now.trunc_subsecs(0);
    if seconds < now {
        seconds + TimeDelta::seconds(1)
    } else {
        seconds
    }
}

/// Describes an exempted spec for the success message.
fn describe_exemption(spec: &str, cutoff: Option<DateTime<Utc>>) -> String {
    match cutoff {
        Some(cutoff) => format!(
            "{spec} (published up to {})",
            pixi_manifest::format_cutoff(cutoff)
        ),
        None => spec.to_string(),
    }
}

/// Exempts the conda `specs` with `add`, with the cutoff of `cutoff`.
/// Returns the success message.
pub(crate) fn add_exemptions(
    specs: &[String],
    cutoff: &ExemptionCutoffArgs,
    channel_config: &ChannelConfig,
    mut add: impl FnMut(PackageName, ExcludeNewerExemption) -> miette::Result<()>,
) -> miette::Result<String> {
    let now = now();
    let mut names = Vec::with_capacity(specs.len());
    for spec in specs {
        let (name, binary) = pixi_manifest::parse_conda_exemption(spec, channel_config)?;
        let cutoff = cutoff.cutoff_for(&binary, now)?;
        names.push(describe_exemption(spec, cutoff));
        add(
            name,
            ExcludeNewerExemption {
                spec: binary,
                cutoff,
            },
        )?;
    }
    Ok(format!("Exempted {} from exclude-newer.", names.join(", ")))
}

/// Removes the `stale` exemptions with `remove`, or only reports them on a
/// dry run. Returns the success message if any exemption was removed.
pub(crate) fn prune_exemptions(
    stale: &[PackageName],
    dry_run: bool,
    mut remove: impl FnMut(&PackageName) -> miette::Result<bool>,
) -> miette::Result<Option<String>> {
    let names = stale.iter().map(PackageName::as_source).join(", ");
    if stale.is_empty() {
        eprintln!("There are no stale exclude-newer exemptions.");
        return Ok(None);
    }
    if dry_run {
        eprintln!("Would remove the stale exclude-newer exemptions of {names}.");
        return Ok(None);
    }
    for name in stale {
        remove(name)?;
    }
    Ok(Some(format!(
        "Removed the stale exclude-newer exemptions of {names}."
    )))
}

/// Prints the success message of a command.
pub(crate) fn print_success(message: &str) {
    eprintln!(
        "{}{message}",
        console::style(console::Emoji("✔ ", "")).green()
    );
}

/// Removes the exemptions of `packages` with `remove`, which returns false if
/// a package was not exempt. Fails with the packages that were not exempt.
pub(crate) fn remove_exemptions<E>(
    packages: &[String],
    target: &'static str,
    mut remove: impl FnMut(&str) -> Result<bool, E>,
) -> Result<(), E>
where
    E: From<ExcludeNewerError>,
{
    let mut missing = Vec::new();
    for package in packages {
        if !remove(package)? {
            missing.push(package.clone());
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(ExcludeNewerError::NotExempt {
            packages: missing,
            target,
        }
        .into())
    }
}

#[derive(Parser, Debug)]
pub struct UpdateArgs {
    /// Edit `pypi-exclude-newer`, which applies to PyPI packages, instead of
    /// `exclude-newer`. Locked PyPI packages are not re-solved, run
    /// `pixi update` for that
    #[clap(long)]
    pub pypi: bool,

    #[clap(flatten)]
    pub lock: LockUpdateArgs,
}

#[derive(Parser, Debug)]
pub struct LockUpdateArgs {
    #[clap(flatten)]
    pub no_install_config: NoInstallConfig,

    #[clap(flatten)]
    pub lock_file_update_config: LockFileUpdateConfig,

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
    /// `conda-forge::ruff`. PyPI exemptions only take package names
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
    let locator = WorkspaceLocator::for_cli()
        .with_global_config_source(args.config_source.source())
        .with_search_start(args.workspace_config.workspace_locator_start());

    let (lock, pypi) = match &args.command {
        None | Some(Command::Get) => {
            let workspace = locator.locate().await?;
            return print_config(&workspace);
        }
        Some(Command::Set(SetArgs { update, .. }))
        | Some(Command::Unset(update))
        | Some(Command::Exempt(ExemptArgs { update, .. }))
        | Some(Command::Unexempt(UnexemptArgs { update, .. })) => (&update.lock, update.pypi),
        Some(Command::Prune(args)) => return prune(locator, args).await,
    };
    let workspace = locator
        .with_cli_config(lock.config.clone())
        .locate()
        .await?;
    let channel_config = workspace.channel_config();
    let mut workspace = workspace.modify()?;
    let original = workspace.document().render().ok();
    let target = if pypi {
        "pypi-exclude-newer"
    } else {
        "exclude-newer"
    };

    let message = match args.command.as_ref().expect("handled above") {
        Command::Get | Command::Prune(_) => unreachable!("handled above"),
        Command::Set(args) => {
            let mut manifest = workspace.manifest();
            if pypi {
                manifest.set_pypi_exclude_newer_cutoff(Some(args.cutoff.trim()))?;
            } else {
                manifest.set_exclude_newer_cutoff(Some(args.cutoff.trim()))?;
            }
            format!("Set the {target} cutoff to '{}'.", args.cutoff.trim())
        }
        Command::Unset(_) => {
            let mut manifest = workspace.manifest();
            if pypi {
                manifest.set_pypi_exclude_newer_cutoff(None)?;
            } else {
                manifest.set_exclude_newer_cutoff(None)?;
            }
            format!("Removed the {target} cutoff.")
        }
        Command::Exempt(args) if pypi => {
            if args.cutoff.is_set() {
                return Err(ExcludeNewerError::PypiExemptionCutoff.into());
            }
            let mut names = Vec::with_capacity(args.specs.len());
            for spec in &args.specs {
                let name = pixi_manifest::parse_pypi_exemption(spec)?;
                names.push(name.as_source().to_string());
                workspace
                    .manifest()
                    .add_pypi_exclude_newer_exemption(name)?;
            }
            format!("Exempted {} from {target}.", names.join(", "))
        }
        Command::Exempt(args) => add_exemptions(
            &args.specs,
            &args.cutoff,
            &channel_config,
            |name, exemption| {
                Ok(workspace
                    .manifest()
                    .add_exclude_newer_exemption(name, exemption)?)
            },
        )?,
        Command::Unexempt(args) => {
            remove_exemptions(&args.packages, target, |package| {
                if pypi {
                    let name =
                        PypiPackageName::from_str(package).map_err(ExcludeNewerError::from)?;
                    Ok::<_, miette::Report>(
                        workspace
                            .manifest()
                            .remove_pypi_exclude_newer_exemption(&name)?,
                    )
                } else {
                    let name = PackageName::from_str(package).map_err(ExcludeNewerError::from)?;
                    Ok(workspace.manifest().remove_exclude_newer_exemption(&name)?)
                }
            })?;
            format!(
                "Removed the {target} exemptions of {}.",
                args.packages.join(", ")
            )
        }
    };

    if workspace.document().render().ok() == original {
        eprintln!("The {target} configuration is unchanged.");
        return Ok(());
    }

    // Like `pixi add`, save Python-backed manifests before updating the lock
    // file so tools like `uv` observe the changes. Dropping `workspace` on an
    // error reverts the manifest.
    if workspace.workspace().workspace.provenance.kind == ManifestKind::Pyproject {
        workspace
            .save_inner()
            .await
            .map_err(ExcludeNewerError::from)?;
    }

    // The lock file is written before the environment is installed, so
    // restore it if a later step fails.
    let lock_file_path = workspace.workspace().lock_file_path();
    let original_lock_file = fs_err::tokio::read(&lock_file_path).await.ok();
    let result = match update_lock_file(&workspace, lock).await {
        Ok(()) => workspace
            .save()
            .await
            .map(drop)
            .map_err(|err| ExcludeNewerError::from(err).into()),
        Err(err) => Err(err),
    };
    if let Err(err) = result {
        restore_lock_file(&lock_file_path, original_lock_file).await;
        return Err(err);
    }

    print_success(&message);
    Ok(())
}

/// Removes the stale exemptions of `exclude-newer` from the manifest.
async fn prune(locator: WorkspaceLocator, args: &PruneArgs) -> miette::Result<()> {
    let workspace = locator
        .with_cli_config(args.config.clone())
        .locate()
        .await?;
    let stale = stale_exemptions(&workspace)?;
    let mut workspace = workspace.modify()?;
    let Some(message) = prune_exemptions(&stale, args.dry_run, |name| {
        Ok(workspace.manifest().remove_exclude_newer_exemption(name)?)
    })?
    else {
        return Ok(());
    };
    workspace.save().await.map_err(ExcludeNewerError::from)?;
    print_success(&message);
    Ok(())
}

/// Restores the lock file to `original`, or removes it if there was none.
async fn restore_lock_file(path: &Path, original: Option<Vec<u8>>) {
    let result = match original {
        Some(contents) => fs_err::tokio::write(path, contents).await,
        None => match fs_err::tokio::remove_file(path).await {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            result => result,
        },
    };
    if let Err(err) = result {
        tracing::error!("Failed to restore {}: {}", path.display(), err);
    }
}

/// Returns the exemptions of `exclude-newer` that are stale for every
/// environment of the workspace.
fn stale_exemptions(workspace: &Workspace) -> miette::Result<Vec<PackageName>> {
    let config = &workspace.workspace.value.workspace;
    let environments = workspace.environments();
    let channels = environments
        .iter()
        .map(|environment| environment.prioritized_channels())
        .collect::<Vec<_>>();
    Ok(pixi_manifest::stale_exemptions(
        &config.exclude_newer,
        channels.iter().map(|channels| channels.values().copied()),
        &workspace.channel_config(),
        &config.exclude_newer_package_overrides,
        Utc::now(),
    )?)
}

/// Updates the lock file, so a stricter cutoff takes effect right away for
/// conda packages. Locked PyPI packages carry no upload time, so they are not
/// re-solved. Like `pixi add`, the default environment is only installed when
/// it is the only environment of the workspace.
async fn update_lock_file(workspace: &WorkspaceMut, update: &LockUpdateArgs) -> miette::Result<()> {
    let progress = workspace.progress().cloned();
    let workspace = workspace.workspace();
    get_update_lock_file_and_prefix(
        &workspace.default_environment(),
        progress,
        UpdateMode::Revalidate,
        UpdateLockFileOptions {
            lock_file_usage: update.lock_file_update_config.lock_file_usage()?,
            no_install: update.no_install_config.no_install || workspace.environments().len() != 1,
            max_concurrent_solves: workspace.config().max_concurrent_solves(),
            ..Default::default()
        },
        ReinstallPackages::default(),
        &InstallFilter::default(),
    )
    .await?;
    Ok(())
}

fn print_config(workspace: &Workspace) -> miette::Result<()> {
    let document = ManifestDocument::from_provenance(&workspace.workspace.provenance)?;
    let entries = ["exclude-newer", "pypi-exclude-newer"]
        .into_iter()
        .filter_map(|key| Some((key, entry_as_written(document.workspace_entry(key)?)?)))
        .collect::<Vec<_>>();
    if entries.is_empty() {
        eprintln!("No exclude-newer configured.");
    }
    for (key, value) in entries {
        println!("{key} = {value}");
    }
    Ok(())
}
