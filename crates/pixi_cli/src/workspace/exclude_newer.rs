use std::str::FromStr;

use clap::Parser;
use pixi_config::ConfigCli;
use pixi_core::{
    InstallFilter, UpdateLockFileOptions, Workspace, WorkspaceLocator,
    environment::get_update_lock_file_and_prefix,
    lock_file::{ReinstallPackages, UpdateMode},
    workspace::WorkspaceMut,
};
use pixi_manifest::{ExcludeNewerConfig, PypiExcludeNewerConfig};
use pixi_pypi_spec::PypiPackageName;
use rattler_conda_types::PackageName;

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
    /// Remove the cutoff.
    ///
    /// The cutoff of `exclude-newer` cannot be removed while it has exemptions.
    Unset(UpdateArgs),
    /// Exempt package releases from the cutoff.
    ///
    /// Exemptions are match specs, a bare package name exempts every release.
    /// An existing exemption of the same package is replaced.
    ///
    /// Example:
    /// `pixi workspace exclude-newer exempt 'python=3.13.*' conda-forge::ruff`
    #[clap(verbatim_doc_comment)]
    Exempt(ExemptArgs),
    /// Remove the exemptions of packages.
    ///
    /// Example:
    /// `pixi workspace exclude-newer unexempt python ruff`
    #[clap(verbatim_doc_comment)]
    Unexempt(UnexemptArgs),
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
    /// `exclude-newer`
    #[clap(long)]
    pub pypi: bool,

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
    let locator = WorkspaceLocator::for_cli()
        .with_global_config_source(args.config_source.source())
        .with_search_start(args.workspace_config.workspace_locator_start());

    let update = match &args.command {
        None | Some(Command::Get) => {
            let workspace = locator.locate().await?;
            print_config(&workspace);
            return Ok(());
        }
        Some(Command::Set(SetArgs { update, .. }))
        | Some(Command::Unset(update))
        | Some(Command::Exempt(ExemptArgs { update, .. }))
        | Some(Command::Unexempt(UnexemptArgs { update, .. })) => update,
    };
    let workspace = locator
        .with_cli_config(update.config.clone())
        .locate()
        .await?;
    let channel_config = workspace.channel_config();
    let original = exclude_newer_config(&workspace);
    let mut workspace = workspace.modify()?;
    let pypi = update.pypi;
    let target = if pypi {
        "pypi-exclude-newer"
    } else {
        "exclude-newer"
    };

    let message = match args.command.as_ref().expect("handled above") {
        Command::Get => unreachable!("handled above"),
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
        Command::Exempt(args) => {
            let mut names = Vec::with_capacity(args.specs.len());
            for spec in &args.specs {
                if pypi {
                    let name = pixi_manifest::parse_pypi_exemption(spec)?;
                    names.push(name.as_source().to_string());
                    workspace
                        .manifest()
                        .add_pypi_exclude_newer_exemption(name)?;
                } else {
                    names.push(spec.clone());
                    let (name, spec) = pixi_manifest::parse_conda_exemption(spec, &channel_config)?;
                    workspace
                        .manifest()
                        .add_exclude_newer_exemption(name, spec)?;
                }
            }
            format!("Exempted {} from {target}.", names.join(", "))
        }
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

    if exclude_newer_config(workspace.workspace()) == original {
        eprintln!("The {target} configuration is unchanged.");
        return Ok(());
    }

    // Save the manifest before updating the lock file, so the lock file is
    // only written once the manifest is on disk. Dropping `workspace` on an
    // error reverts the manifest.
    workspace
        .save_inner()
        .await
        .map_err(ExcludeNewerError::from)?;
    update_lock_file(&workspace, update).await?;
    workspace.save().await.map_err(ExcludeNewerError::from)?;

    eprintln!(
        "{}{message}",
        console::style(console::Emoji("✔ ", "")).green()
    );
    Ok(())
}

fn exclude_newer_config(workspace: &Workspace) -> (ExcludeNewerConfig, PypiExcludeNewerConfig) {
    let config = &workspace.workspace.value.workspace;
    (
        config.exclude_newer.clone(),
        config.pypi_exclude_newer.clone(),
    )
}

/// Updates the lock file, so a stricter cutoff takes effect right away. Like
/// `pixi add`, the default environment is only installed when it is the only
/// environment of the workspace.
async fn update_lock_file(workspace: &WorkspaceMut, update: &UpdateArgs) -> miette::Result<()> {
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

fn print_config(workspace: &Workspace) {
    let config = &workspace.workspace.value.workspace;
    if config.exclude_newer.is_empty() && config.pypi_exclude_newer.is_empty() {
        eprintln!("No exclude-newer configured.");
        return;
    }
    if !config.exclude_newer.is_empty() {
        println!(
            "exclude-newer = {}",
            conda_toml_value(&config.exclude_newer)
        );
    }
    if !config.pypi_exclude_newer.is_empty() {
        println!(
            "pypi-exclude-newer = {}",
            pypi_toml_value(&config.pypi_exclude_newer)
        );
    }
}

pub(crate) fn conda_toml_value(config: &ExcludeNewerConfig) -> toml_edit::Value {
    serde::Serialize::serialize(config, toml_edit::ser::ValueSerializer::new())
        .expect("conversion to toml cannot fail")
}

fn pypi_toml_value(config: &PypiExcludeNewerConfig) -> toml_edit::Value {
    let mut table = toml_edit::InlineTable::new();
    if let Some(cutoff) = &config.cutoff {
        let cutoff = serde::Serialize::serialize(cutoff, toml_edit::ser::ValueSerializer::new())
            .expect("conversion to toml cannot fail");
        table.insert("cutoff", cutoff);
    }
    if !config.exemptions.is_empty() {
        let exemptions = config
            .exemptions
            .keys()
            .map(|name| (name.as_source(), toml_edit::Value::from("*")))
            .collect::<toml_edit::InlineTable>();
        table.insert("exemptions", exemptions.into());
    }
    table.into()
}
