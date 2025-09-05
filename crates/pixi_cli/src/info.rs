use std::{fmt::Display, path::PathBuf};

use chrono::{DateTime, Local};
use clap::Parser;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_config;
use pixi_consts::consts;
use pixi_core::WorkspaceLocator;
use pixi_global::{BinDir, EnvRoot};
use pixi_manifest::{EnvironmentName, FeatureName, SystemRequirements};
use pixi_manifest::{FeaturesExt, HasFeaturesIter};
use pixi_progress::await_in_progress;
use pixi_task::TaskName;
use rattler_conda_types::{GenericVirtualPackage, Platform};
use rattler_networking::authentication_storage;
use rattler_virtual_packages::{VirtualPackage, VirtualPackageOverrides};
use serde::Serialize;
use serde_with::{DisplayFromStr, serde_as};
use tokio::task::spawn_blocking;
use toml_edit::ser::to_string;

use crate::cli_config::WorkspaceConfig;

static WIDTH: usize = 19;

/// Information about the system, workspace and environments for the current machine.
#[derive(Parser, Debug)]
pub struct Args {
    /// Show cache and environment size
    #[arg(long)]
    extended: bool,

    /// Whether to show the output as JSON or not
    #[arg(long)]
    json: bool,

    #[clap(flatten)]
    pub project_config: WorkspaceConfig,
}

#[derive(Serialize)]
pub struct WorkspaceInfo {
    name: String,
    manifest_path: PathBuf,
    last_updated: Option<String>,
    pixi_folder_size: Option<String>,
    version: Option<String>,
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
    system_requirements: SystemRequirements,
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
                consts::SOLVE_GROUP_STYLE.apply_to(solve_group)
            )?;
        }
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

        writeln!(
            f,
            "{:>WIDTH$}: {}",
            bold.apply_to("Prefix location"),
            self.prefix.display()
        )?;

        if !self.system_requirements.is_empty() {
            let serialized = to_string(&self.system_requirements)
                .expect("it should always be possible to convert system requirements to a string");
            let indented = serialized
                .lines()
                .enumerate()
                .map(|(i, line)| {
                    if i == 0 {
                        // First line includes the label
                        format!("{:>WIDTH$}: {}", bold.apply_to("System requirements"), line)
                    } else {
                        // Subsequent lines are indented to align
                        format!("{:>WIDTH$}  {}", "", line)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");

            writeln!(f, "{}", indented)?;
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

/// Information about `pixi global`
#[derive(Serialize)]
struct GlobalInfo {
    bin_dir: PathBuf,
    env_dir: PathBuf,
    manifest: PathBuf,
}
impl Display for GlobalInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bold = console::Style::new().bold();
        writeln!(
            f,
            "{:>WIDTH$}: {}",
            bold.apply_to("Bin dir"),
            self.bin_dir.to_string_lossy()
        )?;
        writeln!(
            f,
            "{:>WIDTH$}: {}",
            bold.apply_to("Environment dir"),
            self.env_dir.to_string_lossy()
        )?;
        writeln!(
            f,
            "{:>WIDTH$}: {}",
            bold.apply_to("Manifest dir"),
            self.manifest.to_string_lossy()
        )?;
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
    global_info: Option<GlobalInfo>,
    project_info: Option<WorkspaceInfo>,
    environments_info: Vec<EnvironmentInfo>,
    config_locations: Vec<PathBuf>,
}
impl Display for Info {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bold = console::Style::new().bold();
        let cache_dir = match &self.cache_dir {
            Some(path) => path.to_string_lossy().to_string(),
            None => "None".to_string(),
        };

        writeln!(f, "{}", bold.apply_to("System\n------------").cyan())?;
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

        let config_locations = self
            .config_locations
            .iter()
            .map(|p| p.to_string_lossy())
            .join(" ");

        writeln!(
            f,
            "{:>WIDTH$}: {}",
            bold.apply_to("Config locations"),
            if config_locations.is_empty() {
                "No config files found"
            } else {
                &config_locations
            }
        )?;

        // Pixi global information
        if let Some(gi) = self.global_info.as_ref() {
            writeln!(f, "\n{}", bold.apply_to("Global\n------------").cyan())?;
            write!(f, "{}", gi)?;
        }

        // Workspace information
        if let Some(pi) = self.project_info.as_ref() {
            writeln!(f, "\n{}", bold.apply_to("Workspace\n------------").cyan())?;
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
            writeln!(
                f,
                "\n{}",
                bold.apply_to("Environments\n------------").cyan()
            )?;
            for e in &self.environments_info {
                writeln!(f, "{}", e)?;
            }
        }

        Ok(())
    }
}

/// Returns the size of a directory
fn dir_size(path: impl Into<PathBuf>) -> miette::Result<String> {
    fn dir_size(mut dir: fs_err::ReadDir) -> miette::Result<u64> {
        dir.try_fold(0, |acc, file| {
            let file = file.into_diagnostic()?;
            let size = match file.metadata().into_diagnostic()? {
                data if data.is_dir() => {
                    dir_size(fs_err::read_dir(file.path()).into_diagnostic()?)?
                }
                data => data.len(),
            };
            Ok(acc + size)
        })
    }

    let size = dir_size(fs_err::read_dir(path.into()).into_diagnostic()?)?;
    Ok(format!("{} MiB", size / 1024 / 1024))
}

/// Returns last update time of file, formatted: DD-MM-YYYY H:M:S
fn last_updated(path: impl Into<PathBuf>) -> miette::Result<String> {
    let time = fs_err::metadata(path.into())
        .into_diagnostic()?
        .modified()
        .into_diagnostic()?;
    let formatted_time = DateTime::<Local>::from(time)
        .format("%d-%m-%Y %H:%M:%S")
        .to_string();

    Ok(formatted_time)
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.project_config.workspace_locator_start())
        .locate()
        .ok();

    let (pixi_folder_size, cache_size) = if args.extended {
        let env_dir = workspace.as_ref().map(|p| p.pixi_dir());
        let cache_dir = pixi_config::get_cache_dir()?;
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

    let project_info = workspace.clone().map(|p| WorkspaceInfo {
        name: p.display_name().to_string(),
        manifest_path: p.workspace.provenance.path.clone(),
        last_updated: last_updated(p.lock_file_path()).ok(),
        pixi_folder_size,
        version: p
            .workspace
            .value
            .workspace
            .version
            .clone()
            .map(|v| v.to_string()),
    });

    let environments_info: Vec<EnvironmentInfo> = workspace
        .as_ref()
        .map(|ws| {
            ws.environments()
                .iter()
                .map(|env| {
                    let tasks = env
                        .tasks(Some(env.best_platform()))
                        .ok()
                        .map(|t| t.into_keys().cloned().collect())
                        .unwrap_or_default();

                    let environment_size =
                        args.extended.then(|| dir_size(env.dir()).ok()).flatten();

                    EnvironmentInfo {
                        name: env.name().clone(),
                        features: env.features().map(|feature| feature.name.clone()).collect(),
                        solve_group: env
                            .solve_group()
                            .map(|solve_group| solve_group.name().to_string()),
                        environment_size,
                        dependencies: env
                            .combined_dependencies(Some(env.best_platform()))
                            .names()
                            .map(|p| p.as_source().to_string())
                            .collect(),
                        pypi_dependencies: env
                            .pypi_dependencies(Some(env.best_platform()))
                            .into_iter()
                            .map(|(name, _p)| name.as_source().to_string())
                            .collect(),
                        platforms: env.platforms().into_iter().collect(),
                        system_requirements: env.system_requirements().clone(),
                        channels: env.channels().into_iter().map(|c| c.to_string()).collect(),
                        prefix: env.dir(),
                        tasks,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let global_info = Some(GlobalInfo {
        bin_dir: BinDir::from_env().await?.path().to_path_buf(),
        env_dir: EnvRoot::from_env().await?.path().to_path_buf(),
        manifest: pixi_global::Project::manifest_dir()?.join(consts::GLOBAL_MANIFEST_DEFAULT_NAME),
    });

    let virtual_packages = VirtualPackage::detect(&VirtualPackageOverrides::from_env())
        .into_diagnostic()?
        .iter()
        .cloned()
        .map(GenericVirtualPackage::from)
        .collect::<Vec<_>>();

    let config = workspace
        .map(|p| p.config().clone())
        .unwrap_or_else(pixi_config::Config::load_global);

    let auth_file: PathBuf = if let Ok(auth_file) = std::env::var("RATTLER_AUTH_FILE") {
        auth_file.into()
    } else if let Some(auth_file) = config.authentication_override_file() {
        auth_file.to_owned()
    } else {
        authentication_storage::backends::file::FileStorage::new()
            .into_diagnostic()?
            .path
    };

    let info = Info {
        platform: Platform::current().to_string(),
        virtual_packages,
        version: consts::PIXI_VERSION.to_string(),
        cache_dir: Some(pixi_config::get_cache_dir()?),
        cache_size,
        auth_dir: auth_file,
        project_info,
        environments_info,
        global_info,
        config_locations: config.loaded_from.clone(),
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&info).into_diagnostic()?);
    } else {
        println!("{}", info);
    }

    Ok(())
}
