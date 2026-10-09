use std::{fmt::Display, io::Write, path::PathBuf, sync::Arc};

use chrono::{DateTime, Local};
use clap::Parser;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_config::DetectorDecision;
use pixi_consts::consts;
use pixi_core::environment::{PlatformData, RequiredPlatform};
use pixi_core::host::{
    DenyAll, DetectedValue, DetectionSource, HostDetection, HostDetector, SkipReason, WantedNames,
};
use pixi_core::{WorkspaceLocator, WorkspaceLocatorError};
use pixi_global::{BinDir, EnvRoot};
use pixi_manifest::platform::{solver_generic_virtual_packages, subdir_default_virtual_packages};
use pixi_manifest::toml::inline_virtual_package_specs;
use pixi_manifest::{EnvironmentName, FeatureName, PixiPlatformName};
use pixi_manifest::{FeaturesExt, HasFeaturesIter, HasWorkspaceManifest};
use pixi_progress::await_in_progress;
use pixi_task::TaskName;
use pixi_utils::reqwest::tls_backend;
use rattler_conda_types::{GenericVirtualPackage, Subdir};
use rattler_networking::authentication_storage;
use rattler_virtual_package_detectors::merge_results;
use serde::Serialize;
use serde_with::{DisplayFromStr, serde_as};
use tokio::task::spawn_blocking;

use crate::cli_config::WorkspaceConfig;

static WIDTH: usize = 19;

/// Information about the system, workspace and environments for the current machine.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,

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
pub struct PlatformInfo {
    name: PixiPlatformName,
    subdir: String,
    /// Friendly `key=value` form, used for both text and `--json`.
    virtual_packages: Vec<String>,
}

/// Render `declared` in the friendly `key=value` form, optionally filtering
/// `baseline` (the subdir defaults).
fn friendly_virtual_packages(
    declared: &[GenericVirtualPackage],
    baseline: Option<&[GenericVirtualPackage]>,
) -> Vec<String> {
    inline_virtual_package_specs(declared, baseline)
        .into_iter()
        .map(|spec| spec.rendered)
        .collect()
}

impl From<&pixi_manifest::PixiPlatform> for PlatformInfo {
    fn from(platform: &pixi_manifest::PixiPlatform) -> Self {
        Self {
            name: platform.name().clone(),
            subdir: platform.subdir().to_string(),
            // Declared platform: filter the subdir defaults, like `platform list`.
            virtual_packages: friendly_virtual_packages(
                platform.declared_virtual_packages(),
                Some(&subdir_default_virtual_packages(platform.subdir())),
            ),
        }
    }
}

impl From<&PlatformData> for PlatformInfo {
    fn from(data: &PlatformData) -> Self {
        Self {
            name: data.subdir().into(),
            subdir: data.subdir().to_string(),
            virtual_packages: friendly_virtual_packages(data.virtual_packages(), None),
        }
    }
}

/// Built from a marker-file [`RequiredPlatform`]. Its entries are match specs
/// rather than concrete virtual packages, so they are shown as written -- a
/// requirement is a constraint, and there is no friendly manifest key for one.
impl From<&RequiredPlatform> for PlatformInfo {
    fn from(data: &RequiredPlatform) -> Self {
        Self {
            name: data.subdir().into(),
            subdir: data.subdir().to_string(),
            virtual_packages: data.requirement_strings(),
        }
    }
}

/// Human-readable representation of a platform entry in the `pixi info`
/// output: bare name when it carries no customised VPs, otherwise
/// `<name> (vp1, vp2, ...)` in friendly form.
fn format_platform(info: &PlatformInfo) -> String {
    if info.virtual_packages.is_empty() {
        info.name.to_string()
    } else {
        format!("{} ({})", info.name, info.virtual_packages.join(", "))
    }
}

#[derive(Serialize)]
pub struct EnvironmentInfo {
    name: EnvironmentName,
    features: Vec<FeatureName>,
    solve_group: Option<String>,
    environment_size: Option<String>,
    dependencies: Vec<String>,
    pypi_dependencies: Vec<String>,
    platforms: Vec<PlatformInfo>,
    resolved_platform: Option<PlatformInfo>,
    minimum_supported_platform: Option<PlatformInfo>,
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
            let platform_list = self.platforms.iter().map(format_platform).format(", ");
            writeln!(
                f,
                "{:>WIDTH$}: {}",
                bold.apply_to("Target platforms"),
                platform_list
            )?;
        }

        if let Some(resolved) = &self.resolved_platform {
            writeln!(
                f,
                "{:>WIDTH$}: {}",
                bold.apply_to("Resolved platform"),
                format_platform(resolved)
            )?;
        }
        // Always shown so users know where to look for the minimum platform;
        // it's only computed once the environment has been installed.
        let minimum = self.minimum_supported_platform.as_ref().map_or_else(
            || {
                console::style("available after `pixi install`")
                    .dim()
                    .to_string()
            },
            format_platform,
        );
        writeln!(
            f,
            "{:>WIDTH$}: {}",
            bold.apply_to("Minimum platform"),
            minimum
        )?;

        writeln!(
            f,
            "{:>WIDTH$}: {}",
            bold.apply_to("Prefix location"),
            self.prefix.display()
        )?;

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

/// One virtual package detector the channels register, and what became of
/// it on this machine.
#[derive(Serialize)]
pub struct DetectorInfo {
    /// The registering channel's base URL.
    origin: String,
    /// The detector package.
    detector: String,
    /// `ran`, `cached`, `failed` or `skipped`.
    state: String,
    /// Why it failed or was skipped.
    detail: Option<String>,
    /// What it reported, as `__name=version=build` or `__name absent`.
    virtual_packages: Vec<String>,
}

impl DetectorInfo {
    /// The detectors of `host` in the order they were reported: those that
    /// ran, then those that failed, then those that were skipped.
    fn from_host(host: &HostDetection, config: &pixi_config::Config) -> Vec<Self> {
        let mut ran: Vec<Self> = Vec::new();
        for result in host.detector_results() {
            let DetectionSource::Detector {
                origin,
                detector,
                from_cache,
                ..
            } = &result.source
            else {
                continue;
            };
            let reported = match &result.value {
                DetectedValue::Absent => format!("{} absent", result.name.as_normalized()),
                DetectedValue::Present(_) => result
                    .virtual_package()
                    .map(|package| package.to_string())
                    .unwrap_or_default(),
            };
            let origin = origin.as_str();
            let detector = detector.as_source();
            match ran
                .iter_mut()
                .find(|info| info.origin == origin && info.detector == detector)
            {
                Some(info) => info.virtual_packages.push(reported),
                None => ran.push(Self {
                    origin: origin.to_string(),
                    detector: detector.to_string(),
                    state: if *from_cache { "cached" } else { "ran" }.to_string(),
                    detail: None,
                    virtual_packages: vec![reported],
                }),
            }
        }
        let failed = host.detector_failures().iter().map(|failure| {
            let detail = match &failure.stderr {
                Some(stderr) if !stderr.trim().is_empty() => {
                    format!("{}\n{}", failure.message, stderr.trim_end())
                }
                _ => failure.message.clone(),
            };
            Self {
                origin: failure.origin.to_string(),
                detector: failure.detector.as_source().to_string(),
                state: "failed".to_string(),
                detail: Some(detail),
                virtual_packages: Vec::new(),
            }
        });
        let skipped = host.skipped_detectors().iter().map(|skipped| {
            let detail = match &skipped.reason {
                SkipReason::TargetIsNotHost { override_variables } => {
                    format!(
                        "the target platform is not this machine's; set {} to supply its virtual packages",
                        override_variables.join(", ")
                    )
                }
                SkipReason::NoWantedName => {
                    "none of its virtual packages is needed, or all are overridden".to_string()
                }
                SkipReason::ConsentDenied => match config
                    .virtual_package_detectors
                    .consent(&skipped.origin)
                {
                    Some(DetectorDecision::Deny) => "denied in the configuration".to_string(),
                    _ => format!(
                        "no decision yet. Trust this channel with `pixi config set --shared \
                         'virtual-package-detectors.consent.{}' allow`, or use `--local` for this repository",
                        toml_edit::Key::new(skipped.origin.as_str().trim_end_matches('/')),
                    ),
                },
            };
            Self {
                origin: skipped.origin.to_string(),
                detector: skipped.detector.as_source().to_string(),
                state: "skipped".to_string(),
                detail: Some(detail),
                virtual_packages: Vec::new(),
            }
        });
        ran.into_iter().chain(failed).chain(skipped).collect()
    }
}

impl Display for DetectorInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}: {}", self.origin, self.detector, self.state)?;
        if !self.virtual_packages.is_empty() {
            write!(f, ", {}", self.virtual_packages.join(", "))?;
        }
        if let Some(detail) = &self.detail {
            write!(f, " ({detail})")?;
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
    virtual_package_detectors: Vec<DetectorInfo>,
    version: String,
    tls_backend: String,
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
            bold.apply_to("TLS backend"),
            self.tls_backend
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
        for (i, detector) in self.virtual_package_detectors.iter().enumerate() {
            if i == 0 {
                writeln!(f, "{:>WIDTH$}: {}", bold.apply_to("Detectors"), detector)?;
            } else {
                writeln!(f, "{:>WIDTH$}: {}", "", detector)?;
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
            write!(f, "{gi}")?;
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
                writeln!(f, "{e}")?;
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

/// Detects the host with every consented detector: those of the workspace's
/// channels, or of the configured default channels outside a workspace.
async fn detect_host(
    workspace: Option<&pixi_core::Workspace>,
    config: &pixi_config::Config,
) -> miette::Result<HostDetection> {
    let detected = match workspace {
        Some(workspace) => workspace
            .detect_host_with_all_detectors()
            .await
            .map_err(miette::Report::new),
        None => {
            let channel_config = config.global_channel_config();
            let channels = config
                .default_channels()
                .into_iter()
                .map(|channel| channel.into_base_url(channel_config))
                .collect::<Result<Vec<_>, _>>()
                .into_diagnostic()?;
            HostDetector::new(config.clone(), Arc::new(DenyAll), None)?
                .detect(&channels, WantedNames::All)
                .await
                .map_err(miette::Report::new)
        }
    }?;
    // A probe that found no registrations may still have learned about a
    // skipped detector while locating the workspace.
    Ok(match workspace {
        Some(workspace)
            if detected.detector_results().is_empty()
                && detected.detector_failures().is_empty()
                && detected.skipped_detectors().is_empty() =>
        {
            workspace.host().clone()
        }
        _ => detected,
    })
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let source = args.config_source.source();
    let workspace = match WorkspaceLocator::for_cli()
        .with_global_config_source(source.clone())
        .with_search_start(args.project_config.workspace_locator_start())
        .locate()
        .await
    {
        Ok(workspace) => Some(workspace),
        Err(error @ WorkspaceLocatorError::HostDetection(_)) => return Err(error.into()),
        Err(_) => None,
    };

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
                    let best = env.best_declared_platform();
                    let tasks = env
                        .tasks(best)
                        .ok()
                        .map(|t| t.into_keys().cloned().collect())
                        .unwrap_or_default();

                    let environment_size =
                        args.extended.then(|| dir_size(env.dir()).ok()).flatten();

                    let (resolved_platform, minimum_supported_platform) = env.installed_platforms();

                    EnvironmentInfo {
                        name: env.name().clone(),
                        features: env
                            .features()
                            .map(|feature| feature.name.clone())
                            .filter(|name| !name.is_environment())
                            .collect(),
                        solve_group: env
                            .solve_group()
                            .map(|solve_group| solve_group.name().to_string()),
                        environment_size,
                        dependencies: env
                            .combined_dependencies(best)
                            .names()
                            .map(|p| p.as_source().to_string())
                            .collect(),
                        pypi_dependencies: env
                            .pypi_dependencies(best)
                            .into_iter()
                            .map(|(name, _p)| name.as_source().to_string())
                            .collect(),
                        platforms: env
                            .platforms()
                            .iter()
                            .filter_map(|name| {
                                env.workspace_manifest()
                                    .workspace
                                    .platform_by_name(name)
                                    .map(PlatformInfo::from)
                            })
                            .collect(),
                        resolved_platform: resolved_platform.as_ref().map(PlatformInfo::from),
                        minimum_supported_platform: minimum_supported_platform
                            .as_ref()
                            .map(PlatformInfo::from),
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

    let config = workspace
        .as_ref()
        .map(|p| p.config().clone())
        .unwrap_or_else(|| pixi_config::Config::load_global_with(&source));

    let host = detect_host(workspace.as_ref(), &config).await?;
    let virtual_packages = merge_results(
        solver_generic_virtual_packages(host.platform().into_diagnostic()?),
        host.detector_results(),
    );
    let virtual_package_detectors = DetectorInfo::from_host(&host, &config);

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
        platform: Subdir::current().unwrap_or(Subdir::NoArch).to_string(),
        virtual_packages,
        virtual_package_detectors,
        version: consts::PIXI_VERSION.to_string(),
        tls_backend: tls_backend().to_string(),
        cache_dir: Some(pixi_config::get_cache_dir()?),
        cache_size,
        auth_dir: auth_file,
        project_info,
        environments_info,
        global_info,
        config_locations: config.loaded_from.clone(),
    };

    if args.json {
        pixi_utils::io::ignore_broken_pipe(writeln!(
            std::io::stdout(),
            "{}",
            serde_json::to_string_pretty(&info).into_diagnostic()?
        ))
        .into_diagnostic()?;
    } else {
        pixi_utils::io::ignore_broken_pipe(writeln!(std::io::stdout(), "{info}"))
            .into_diagnostic()?;
    }

    Ok(())
}
