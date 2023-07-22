use std::{fmt::Display, fs, path::PathBuf};

use chrono::{DateTime, Local};
use clap::Parser;
use miette::IntoDiagnostic;
use rattler_conda_types::{GenericVirtualPackage, Platform};
use rattler_virtual_packages::VirtualPackage;
use serde::Serialize;
use serde_with::serde_as;
use serde_with::DisplayFromStr;
use tokio::task::spawn_blocking;

use crate::progress::await_in_progress;
use crate::{cli::auth::get_default_auth_store_location, Project};

/// Information about the system and project
#[derive(Parser, Debug)]
pub struct Args {
    /// Show cache and environment size
    #[arg(long)]
    extended: bool,

    /// Whether to show the output as JSON or not
    #[arg(long)]
    json: bool,

    /// The path to 'pixi.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,
}

#[derive(Serialize)]
pub struct ProjectInfo {
    tasks: Vec<String>,
    manifest_path: PathBuf,
    package_count: Option<u64>,
    environment_size: Option<String>,
    last_updated: Option<String>,
    platforms: Vec<Platform>,
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
}

impl Display for Info {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cache_dir = match &self.cache_dir {
            Some(path) => path.to_string_lossy().to_string(),
            None => "None".to_string(),
        };

        writeln!(f, "pixi {}\n", self.version)?;
        writeln!(f, "{:20}: {}", "Platform", self.platform)?;

        for (i, p) in self.virtual_packages.iter().enumerate() {
            if i == 0 {
                writeln!(f, "{:20}: {}", "Virtual packages", p)?;
            } else {
                writeln!(f, "{:20}: {}", "", p)?;
            }
        }

        writeln!(f, "{:20}: {}", "Cache dir", cache_dir)?;

        if let Some(cache_size) = &self.cache_size {
            writeln!(f, "{:20}: {}", "Cache size", cache_size)?;
        }

        writeln!(
            f,
            "{:20}: {}",
            "Auth storage",
            self.auth_dir.to_string_lossy()
        )?;

        if let Some(pi) = self.project_info.as_ref() {
            writeln!(f, "\nProject\n------------\n")?;

            writeln!(
                f,
                "{:20}: {}",
                "Manifest file",
                pi.manifest_path.to_string_lossy()
            )?;

            if let Some(count) = pi.package_count {
                writeln!(f, "{:20}: {}", "Dependency count", count)?;
            }

            if let Some(size) = &pi.environment_size {
                writeln!(f, "{:20}: {}", "Environment size", size)?;
            }

            if let Some(update_time) = &pi.last_updated {
                writeln!(f, "{:20}: {}", "Last updated", update_time)?;
            }

            if !pi.platforms.is_empty() {
                for (i, p) in pi.platforms.iter().enumerate() {
                    if i == 0 {
                        writeln!(f, "{:20}: {}", "Target platforms", p)?;
                    } else {
                        writeln!(f, "{:20}: {}", "", p)?;
                    }
                }
            }

            if !pi.tasks.is_empty() {
                for (i, t) in pi.tasks.iter().enumerate() {
                    if i == 0 {
                        writeln!(f, "{:20}: {}", "Tasks", t)?;
                    } else {
                        writeln!(f, "{:20}: {}", "", t)?;
                    }
                }
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
    let formated_time = DateTime::<Local>::from(time)
        .format("%d-%m-%Y %H:%M:%S")
        .to_string();

    Ok(formated_time)
}

/// Returns number of dependencies on current platform
fn dependency_count(project: &Project) -> miette::Result<u64> {
    let dep_count = project
        .all_dependencies(Platform::current())?
        .keys()
        .cloned()
        .fold(0, |acc, _| acc + 1);

    Ok(dep_count)
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.manifest_path.as_deref()).ok();

    let cache_dir = rattler::default_cache_dir()
        .map_err(|_| miette::miette!("Could not determine default cache directory"))?;
    let (environment_size, cache_size) = if args.extended {
        let cache_dir = cache_dir.clone();
        let env_dir = project.as_ref().map(|p| p.root().join(".pixi"));
        await_in_progress(
            "fetching cache",
            spawn_blocking(move || {
                let env_size = env_dir.and_then(|env| dir_size(env).ok());
                let cache_size = dir_size(cache_dir).ok();
                (env_size, cache_size)
            }),
        )
        .await
        .into_diagnostic()?
    } else {
        (None, None)
    };

    let project_info = project.map(|p| ProjectInfo {
        manifest_path: p.root().to_path_buf().join("pixi.toml"),
        tasks: p.manifest.tasks.keys().cloned().collect(),
        package_count: dependency_count(&p).ok(),
        environment_size,
        last_updated: last_updated(p.lock_file_path()).ok(),
        platforms: p.platforms().to_vec(),
    });

    let virtual_packages = VirtualPackage::current()
        .into_diagnostic()?
        .iter()
        .cloned()
        .map(GenericVirtualPackage::from)
        .collect::<Vec<_>>();

    let info = Info {
        platform: Platform::current().to_string(),
        virtual_packages,
        version: env!("CARGO_PKG_VERSION").to_string(),
        cache_dir: Some(cache_dir),
        cache_size,
        auth_dir: get_default_auth_store_location(),
        project_info,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&info).into_diagnostic()?);
        Ok(())
    } else {
        println!("{}", info);
        Ok(())
    }
}
