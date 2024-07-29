use std::{fmt::Display, fs, path::PathBuf};

use chrono::{DateTime, Local};
use clap::Parser;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_manifest::{EnvironmentName, FeatureName};
use rattler_conda_types::{GenericVirtualPackage, Platform};
use rattler_networking::authentication_storage;
use rattler_virtual_packages::VirtualPackage;
use serde::Serialize;
use serde_with::{serde_as, DisplayFromStr};
use tokio::task::spawn_blocking;

use crate::cli::cli_config::ProjectConfig;

use crate::{
    config, consts, fancy_display::FancyDisplay, progress::await_in_progress,
    project::has_features::HasFeatures, task::TaskName, Project,
};

static WIDTH: usize = 18;

/// Information about the system, project and environments for the current
/// machine.
#[derive(Parser, Debug)]
pub struct Args {
    /// Show cache and environment size
    #[arg(long)]
    extended: bool,

    /// Whether to show the output as JSON or not
    #[arg(long)]
    json: bool,

    #[clap(flatten)]
    pub project_config: ProjectConfig,
}

#[derive(Serialize)]
pub struct ProjectInfo {
    name: String,
    manifest_path: PathBuf,
    last_updated: Option<String>,
    pixi_folder_size: Option<String>,
    version: Option<String>,
    configuration: Vec<PathBuf>,
}

#[derive(Serialize)]
pub struct EnvironmentInfo {
    name: EnvironmentName,
    features: Vec<FeatureName>,
    solve_group: Option<String>,
    environment_size: Option<String>,
    dependencies: Vec<String>,
    pypi_dependencies: Vec<String>,
    platforms: Vec<Platform>,
    tasks: Vec<TaskName>,
    channels: Vec<String>,
    prefix: PathBuf,
}

impl Display for EnvironmentInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bold = console::Style::new().bold();
        writeln!(
            f,
            "{:>WIDTH$}: {}",
            bold.apply_to("Environment"),
            self.name.fancy_display().bold()
        )?;
        writeln!(
            f,
            "{:>WIDTH$}: {}",
            bold.apply_to("Features"),
            self.features
                .iter()
                .map(|feature| feature.fancy_display())
                .format(", ")
        )?;
        if let Some(solve_group) = &self.solve_group {
            writeln!(
                f,
                "{:>WIDTH$}: {}",
                bold.apply_to("Solve group"),
                solve_group
            )?;
        }
        // TODO: add environment size when PR 674 is merged
        if let Some(size) = &self.environment_size {
            writeln!(f, "{:>WIDTH$}: {}", bold.apply_to("Environment size"), size)?;
        }
        if !self.channels.is_empty() {
            let channels_list = self.channels.iter().format(", ");
            writeln!(
                f,
                "{:>WIDTH$}: {}",
                bold.apply_to("Channels"),
                channels_list
            )?;
        }
        writeln!(
            f,
            "{:>WIDTH$}: {}",
            bold.apply_to("Dependency count"),
            self.dependencies.len()
        )?;
        if !self.dependencies.is_empty() {
            let dependencies_list = self.dependencies.iter().map(|d| d.to_string()).format(", ");
            writeln!(
                f,
                "{:>WIDTH$}: {}",
                bold.apply_to("Dependencies"),
                dependencies_list
            )?;
        }

        if !self.pypi_dependencies.is_empty() {
            let dependencies_list = self
                .pypi_dependencies
                .iter()
                .map(|d| d.to_string())
                .format(", ");
            writeln!(
                f,
                "{:>WIDTH$}: {}",
                bold.apply_to("PyPI Dependencies"),
                dependencies_list
            )?;
        }

        if !self.platforms.is_empty() {
            let platform_list = self.platforms.iter().map(|p| p.to_string()).format(", ");
            writeln!(
                f,
                "{:>WIDTH$}: {}",
                bold.apply_to("Target platforms"),
                platform_list
            )?;
        }
        if !self.tasks.is_empty() {
            let tasks_list = self
                .tasks
                .iter()
                .filter_map(|t| {
                    if !t.as_str().starts_with('_') {
                        Some(t.fancy_display())
                    } else {
                        None
                    }
                })
                .format(", ");
            writeln!(f, "{:>WIDTH$}: {}", bold.apply_to("Tasks"), tasks_list)?;
        }
        Ok(())
    }
}

#[serde_as]
#[derive(Serialize)]
pub struct Info {
    platform: String,
    #[serde_as(as = "Vec<DisplayFromStr>")]
    virtual_packages: Vec<GenericVirtualPackage>,
    version: String,
    cache_dir: Option<PathBuf>,
    cache_size: Option<String>,
    auth_dir: PathBuf,
    project_info: Option<ProjectInfo>,
    environments_info: Vec<EnvironmentInfo>,
}
impl Display for Info {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bold = console::Style::new().bold();
        let cache_dir = match &self.cache_dir {
            Some(path) => path.to_string_lossy().to_string(),
            None => "None".to_string(),
        };

        writeln!(
            f,
            "{:>WIDTH$}: {}",
            bold.apply_to("Pixi version"),
            console::style(&self.version).green()
        )?;
        writeln!(
            f,
            "{:>WIDTH$}: {}",
            bold.apply_to("Platform"),
            self.platform
        )?;

        for (i, p) in self.virtual_packages.iter().enumerate() {
            if i == 0 {
                writeln!(f, "{:>WIDTH$}: {}", bold.apply_to("Virtual packages"), p)?;
            } else {
                writeln!(f, "{:>WIDTH$}: {}", "", p)?;
            }
        }

        writeln!(f, "{:>WIDTH$}: {}", bold.apply_to("Cache dir"), cache_dir)?;
        if let Some(cache_size) = &self.cache_size {
            writeln!(f, "{:>WIDTH$}: {}", bold.apply_to("Cache size"), cache_size)?;
        }

        writeln!(
            f,
            "{:>WIDTH$}: {}",
            bold.apply_to("Auth storage"),
            self.auth_dir.to_string_lossy()
        )?;

        if let Some(pi) = self.project_info.as_ref() {
            writeln!(f, "\n{}", bold.apply_to("Project\n------------"))?;
            writeln!(f, "{:>WIDTH$}: {}", bold.apply_to("Name"), pi.name)?;
            if let Some(version) = pi.version.clone() {
                writeln!(f, "{:>WIDTH$}: {}", bold.apply_to("Version"), version)?;
            }
            writeln!(
                f,
                "{:>WIDTH$}: {}",
                bold.apply_to("Manifest file"),
                pi.manifest_path.to_string_lossy()
            )?;

            let config_locations = pi
                .configuration
                .iter()
                .map(|p| p.to_string_lossy())
                .join(", ");

            writeln!(
                f,
                "{:>WIDTH$}: {}",
                bold.apply_to("Config locations"),
                config_locations
            )?;

            if let Some(update_time) = &pi.last_updated {
                writeln!(
                    f,
                    "{:>WIDTH$}: {}",
                    bold.apply_to("Last updated"),
                    update_time
                )?;
            }
        }

        if !self.environments_info.is_empty() {
            writeln!(f, "\n{}", bold.apply_to("Environments\n------------"))?;
            for e in &self.environments_info {
                writeln!(f, "{}", e)?;
            }
        }

        Ok(())
    }
}

/// Returns the size of a directory
fn dir_size(path: impl Into<PathBuf>) -> miette::Result<String> {
    fn dir_size(mut dir: fs::ReadDir) -> miette::Result<u64> {
        dir.try_fold(0, |acc, file| {
            let file = file.into_diagnostic()?;
            let size = match file.metadata().into_diagnostic()? {
                data if data.is_dir() => dir_size(fs::read_dir(file.path()).into_diagnostic()?)?,
                data => data.len(),
            };
            Ok(acc + size)
        })
    }

    let size = dir_size(fs::read_dir(path.into()).into_diagnostic()?)?;
    Ok(format!("{} MiB", size / 1024 / 1024))
}

/// Returns last update time of file, formatted: DD-MM-YYYY H:M:S
fn last_updated(path: impl Into<PathBuf>) -> miette::Result<String> {
    let time = fs::metadata(path.into())
        .into_diagnostic()?
        .modified()
        .into_diagnostic()?;
    let formatted_time = DateTime::<Local>::from(time)
        .format("%d-%m-%Y %H:%M:%S")
        .to_string();

    Ok(formatted_time)
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.project_config.manifest_path.as_deref()).ok();

    let (pixi_folder_size, cache_size) = if args.extended {
        let env_dir = project.as_ref().map(|p| p.pixi_dir());
        let cache_dir = config::get_cache_dir()?;
        await_in_progress("fetching directory sizes", |_| {
            spawn_blocking(move || {
                let env_size = env_dir.and_then(|env| dir_size(env).ok());
                let cache_size = dir_size(cache_dir).ok();
                (env_size, cache_size)
            })
        })
        .await
        .into_diagnostic()?
    } else {
        (None, None)
    };

    let project_info = project.clone().map(|p| ProjectInfo {
        name: p.name().to_string(),
        manifest_path: p.manifest_path(),
        last_updated: last_updated(p.lock_file_path()).ok(),
        pixi_folder_size,
        configuration: p.config().loaded_from.clone(),
        version: p.version().clone().map(|v| v.to_string()),
    });

    let environments_info: Vec<EnvironmentInfo> = project
        .as_ref()
        .map(|p| {
            p.environments()
                .iter()
                .map(|env| {
                    let tasks = env
                        .tasks(None)
                        .ok()
                        .map(|t| t.into_keys().cloned().collect())
                        .unwrap_or_default();

                    EnvironmentInfo {
                        name: env.name().clone(),
                        features: env.features().map(|feature| feature.name.clone()).collect(),
                        solve_group: env
                            .solve_group()
                            .map(|solve_group| solve_group.name().to_string()),
                        environment_size: None,
                        dependencies: env
                            .dependencies(None, Some(env.best_platform()))
                            .names()
                            .map(|p| p.as_source().to_string())
                            .collect(),
                        pypi_dependencies: env
                            .pypi_dependencies(Some(env.best_platform()))
                            .into_iter()
                            .map(|(name, _p)| name.as_source().to_string())
                            .collect(),
                        platforms: env.platforms().into_iter().collect(),
                        channels: env.channels().into_iter().map(|c| c.to_string()).collect(),
                        prefix: env.dir(),
                        tasks,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let virtual_packages = VirtualPackage::current()
        .into_diagnostic()?
        .iter()
        .cloned()
        .map(GenericVirtualPackage::from)
        .collect::<Vec<_>>();

    let config = project
        .map(|p| p.config().clone())
        .unwrap_or_else(config::Config::load_global);

    let auth_file = config
        .authentication_override_file()
        .map(|x| x.to_owned())
        .unwrap_or_else(|| {
            authentication_storage::backends::file::FileStorage::default()
                .path
                .clone()
        });

    let info = Info {
        platform: Platform::current().to_string(),
        virtual_packages,
        version: consts::PIXI_VERSION.to_string(),
        cache_dir: Some(config::get_cache_dir()?),
        cache_size,
        auth_dir: auth_file,
        project_info,
        environments_info,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&info).into_diagnostic()?);

        Project::warn_on_discovered_from_env(args.project_config.manifest_path.as_deref());
        Ok(())
    } else {
        println!("{}", info);

        Project::warn_on_discovered_from_env(args.project_config.manifest_path.as_deref());
        Ok(())
    }
}
