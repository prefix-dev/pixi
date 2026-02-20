use std::{
    io::{self, Write},
    str::FromStr,
};

use clap::Parser;
use indexmap::IndexMap;
use miette::{IntoDiagnostic, Report};
use pixi_api::{DefaultContext, WorkspaceContext};
use pixi_config::default_channel_config;
use pixi_core::{WorkspaceLocator, workspace::WorkspaceLocatorError};
use pixi_manifest::FeaturesExt;
use pixi_progress::await_in_progress;
use rattler_conda_types::{PackageName, Platform, RepoDataRecord};
use tracing::{debug, error};
use url::Url;

use crate::{
    cli_config::{ChannelsConfig, WorkspaceConfig},
    cli_interface::CliInterface,
};

/// Search a conda package
///
/// Its output will list the latest version of package.
#[derive(Debug, Parser)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    /// MatchSpec of a package to search
    #[arg(required = true)]
    pub package: String,

    #[clap(flatten)]
    pub channels: ChannelsConfig,

    #[clap(flatten)]
    pub project_config: WorkspaceConfig,

    /// The platform(s) to search for
    #[arg(short, long, default_values_t = [Platform::current(), Platform::NoArch])]
    pub platform: Vec<Platform>,

    /// Search across all platforms (from manifest if available, otherwise all known platforms)
    #[arg(long, conflicts_with = "platform")]
    pub all_platforms: bool,

    /// Limit the number of versions shown per package, -1 for no limit
    #[clap(short, long, default_value = "5", allow_hyphen_values = true)]
    pub limit: i64,

    /// Limit the number of packages shown, -1 for no limit
    #[clap(
        short = 'n',
        long = "limit-packages",
        default_value = "5",
        allow_hyphen_values = true
    )]
    pub limit_packages: i64,
}

pub async fn execute_impl<W: Write>(
    args: Args,
    out: &mut W,
) -> miette::Result<Option<Vec<RepoDataRecord>>> {
    let workspace = match WorkspaceLocator::for_cli()
        .with_search_start(args.project_config.workspace_locator_start())
        .locate()
    {
        Ok(project) => Some(project),
        Err(WorkspaceLocatorError::WorkspaceNotFound(_)) => {
            debug!("No project file found, continuing without project configuration.",);
            None
        }
        Err(err) => {
            error!(
                "Error loading project configuration, continuing without:\n{:?}",
                Report::from(err)
            );
            None
        }
    };

    // Resolve channels from project / CLI args
    let channels = args.channels.resolve_from_project(workspace.as_ref())?;
    eprintln!(
        "Using channels: {}",
        channels
            .iter()
            .map(|c| c.name())
            .collect::<Vec<_>>()
            .join(", ")
    );

    // Resolve platforms
    let platforms = if args.all_platforms {
        if let Some(ref workspace) = workspace {
            let mut platforms: Vec<Platform> = workspace
                .default_environment()
                .platforms()
                .into_iter()
                .collect();
            if !platforms.contains(&Platform::NoArch) {
                platforms.push(Platform::NoArch);
            }
            platforms
        } else {
            Platform::all().collect()
        }
    } else {
        args.platform
    };

    let packages = if let Some(workspace) = workspace {
        await_in_progress("searching packages...", |_| async {
            WorkspaceContext::new(CliInterface {}, workspace)
                .search(&args.package, channels, platforms)
                .await
        })
        .await?
    } else {
        await_in_progress("searching packages...", |_| async {
            DefaultContext::new(CliInterface {})
                .search(&args.package, channels, platforms)
                .await
        })
        .await?
    };

    let limit_versions = if args.limit < 0 {
        None
    } else {
        Some(args.limit as usize)
    };
    let limit_packages = if args.limit_packages < 0 {
        None
    } else {
        Some(args.limit_packages as usize)
    };

    // Print search results with detailed info for first N packages
    if let Err(e) = print_search_results(&packages, out, limit_packages, limit_versions)
        && e.kind() != std::io::ErrorKind::BrokenPipe
    {
        return Err(e).into_diagnostic();
    }

    Ok(Some(packages))
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let mut out = io::stdout();
    execute_impl(args, &mut out).await?;
    Ok(())
}

fn print_search_results<W: Write>(
    packages: &[RepoDataRecord],
    out: &mut W,
    limit_packages: Option<usize>,
    limit_versions: Option<usize>,
) -> io::Result<()> {
    // Group packages by name
    let mut by_name: IndexMap<&PackageName, Vec<&RepoDataRecord>> = IndexMap::new();
    for pkg in packages {
        by_name
            .entry(&pkg.package_record.name)
            .or_default()
            .push(pkg);
    }

    let channel_config = default_channel_config();

    // Single package name => show detailed view
    if by_name.len() == 1 {
        let (_, records) = by_name.iter().next().unwrap();
        let newest = records
            .last()
            .expect("records is non-empty since packages is non-empty");
        let others: Vec<_> = records.iter().rev().skip(1).cloned().collect();
        print_package_info(newest, &others, out, limit_versions)?;
        return Ok(());
    }

    // Multiple package names => show compact summary view
    let n_packages = limit_packages.unwrap_or(usize::MAX);
    let n_versions = limit_versions.unwrap_or(usize::MAX);

    for (i, (name, records)) in by_name.iter().enumerate() {
        if i >= n_packages {
            break;
        }
        if i > 0 {
            writeln!(out)?;
        }

        let total_versions = records.len();
        writeln!(
            out,
            "{} ({} {})",
            console::style(name.as_source()).cyan().bright(),
            total_versions,
            if total_versions == 1 {
                "version"
            } else {
                "versions"
            }
        )?;

        let shown = total_versions.min(n_versions);
        for record in records.iter().rev().take(shown) {
            let channel_name = record
                .channel
                .as_ref()
                .and_then(|channel| Url::from_str(channel).ok())
                .and_then(|url| channel_config.strip_channel_alias(&url))
                .or_else(|| record.channel.clone())
                .unwrap_or_else(|| "<unknown>".to_string());

            writeln!(
                out,
                "  {} {} [{}] {}",
                record.package_record.version,
                record.package_record.build,
                record.package_record.subdir,
                channel_name,
            )?;
        }

        let remaining_versions = total_versions.saturating_sub(shown);
        if remaining_versions > 0 {
            writeln!(
                out,
                "  {} and {} more {} (use -l to show more)",
                console::style("...").dim(),
                remaining_versions,
                if remaining_versions == 1 {
                    "version"
                } else {
                    "versions"
                }
            )?;
        }
    }

    let remaining_packages = by_name.len().saturating_sub(n_packages);
    if remaining_packages > 0 {
        writeln!(
            out,
            "\n{} and {} more {} (use -n to show more)",
            console::style("...").dim(),
            remaining_packages,
            if remaining_packages == 1 {
                "package"
            } else {
                "packages"
            }
        )?;
    }

    Ok(())
}

fn format_additional_builds_string(builds: Option<Vec<&RepoDataRecord>>) -> String {
    let builds = builds.unwrap_or_default();
    match builds.len() {
        0 => String::new(),
        1 => " (+ 1 build)".to_string(),
        _ => format!(" (+ {} builds)", builds.len()),
    }
}

fn print_package_info<W: Write>(
    package: &RepoDataRecord,
    other_versions: &Vec<&RepoDataRecord>,
    out: &mut W,
    limit_versions: Option<usize>,
) -> io::Result<()> {
    writeln!(out)?;

    let package = package.clone();
    let package_name = package.package_record.name.as_source();
    let build = &package.package_record.build;
    let mut grouped_by_version = IndexMap::new();
    for version in other_versions {
        grouped_by_version
            .entry(&version.package_record.version)
            .or_insert_with(Vec::new)
            .insert(0, *version);
    }
    let other_builds = grouped_by_version.shift_remove(&package.package_record.version);
    let package_info = format!(
        "{}-{}-{}{}",
        console::style(package.package_record.name.as_source()),
        console::style(package.package_record.version.to_string()),
        console::style(&package.package_record.build),
        console::style(format_additional_builds_string(other_builds))
    );

    writeln!(out, "{package_info}")?;
    writeln!(out, "{}\n", "-".repeat(package_info.chars().count()))?;

    writeln!(
        out,
        "{:19} {:19}",
        console::style("Name"),
        console::style(package_name)
    )?;

    writeln!(
        out,
        "{:19} {:19}",
        console::style("Version"),
        console::style(package.package_record.version)
    )?;

    writeln!(
        out,
        "{:19} {:19}",
        console::style("Build"),
        console::style(build)
    )?;

    let size = match package.package_record.size {
        Some(size) => size.to_string(),
        None => String::from("Not found."),
    };
    writeln!(
        out,
        "{:19} {:19}",
        console::style("Size"),
        console::style(size)
    )?;

    let license = match package.package_record.license {
        Some(license) => license,
        None => String::from("Not found."),
    };
    writeln!(
        out,
        "{:19} {:19}",
        console::style("License"),
        console::style(license)
    )?;

    writeln!(
        out,
        "{:19} {:19}",
        console::style("Subdir"),
        console::style(package.package_record.subdir)
    )?;

    writeln!(
        out,
        "{:19} {:19}",
        console::style("File Name"),
        console::style(package.identifier.to_file_name())
    )?;

    writeln!(
        out,
        "{:19} {:19}",
        console::style("URL"),
        console::style(package.url)
    )?;

    let md5 = match package.package_record.md5 {
        Some(md5) => format!("{md5:x}"),
        None => "Not available".to_string(),
    };
    writeln!(
        out,
        "{:19} {:19}",
        console::style("MD5"),
        console::style(md5)
    )?;

    let sha256 = match package.package_record.sha256 {
        Some(sha256) => format!("{sha256:x}"),
        None => "Not available".to_string(),
    };
    writeln!(
        out,
        "{:19} {:19}",
        console::style("SHA256"),
        console::style(sha256),
    )?;

    writeln!(out, "\nDependencies:")?;
    for dependency in package.package_record.depends {
        writeln!(out, " - {dependency}")?;
    }

    if let Some(run_exports) = package.package_record.run_exports.as_ref() {
        writeln!(out, "\nRun exports:")?;
        let mut print_run_exports = |name: &str, run_exports: &[String]| {
            if !run_exports.is_empty() {
                writeln!(out, "  {name}:")?;
                for run_export in run_exports {
                    writeln!(out, "   - {run_export}")?;
                }
            }
            Ok::<(), std::io::Error>(())
        };
        print_run_exports("noarch", &run_exports.noarch)?;
        print_run_exports("strong", &run_exports.strong)?;
        print_run_exports("weak", &run_exports.weak)?;
        print_run_exports("strong constrains", &run_exports.strong_constrains)?;
        print_run_exports("weak constrains", &run_exports.weak_constrains)?;
    } else {
        writeln!(out, "\nRun exports: not available in repodata")?;
    }

    // Print summary of older versions for package
    if !grouped_by_version.is_empty() {
        writeln!(out, "\nOther Versions ({}):", grouped_by_version.len())?;
        let version_width = grouped_by_version
            .keys()
            .map(|v| v.to_string().len())
            .chain(["Version".len()].iter().cloned())
            .max()
            .expect("there is at least one version, so this should not be empty")
            + 1;
        let build_width = other_versions
            .iter()
            .map(|v| v.package_record.build.len())
            .chain(["Build".len()].iter().cloned())
            .max()
            .expect("there is at least one build, so this should not be empty")
            + 1;
        writeln!(
            out,
            "{:version_width$} {:build_width$}",
            console::style("Version").bold(),
            console::style("Build").bold(),
            version_width = version_width,
            build_width = build_width
        )?;
        let max_displayed = limit_versions.unwrap_or(usize::MAX);
        for (i, (version, builds)) in grouped_by_version.iter().enumerate() {
            if i >= max_displayed {
                let remaining = grouped_by_version.len() - max_displayed;
                writeln!(
                    out,
                    "{} and {} more",
                    console::style("...").dim(),
                    remaining
                )?;
                break;
            }
            writeln!(
                out,
                "{:version_width$} {:build_width$}{}",
                console::style(version.to_string()),
                console::style(builds[0].package_record.build.clone()),
                console::style(format_additional_builds_string(Some(builds[1..].to_vec()))).dim(),
                version_width = version_width,
                build_width = build_width
            )?;
        }
    }

    Ok(())
}
