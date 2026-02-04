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
    /// Name of package to search
    #[arg(required = true)]
    pub package: String,

    #[clap(flatten)]
    pub channels: ChannelsConfig,

    #[clap(flatten)]
    pub project_config: WorkspaceConfig,

    /// The platform to search for, defaults to current platform
    #[arg(short, long, default_value_t = Platform::current())]
    pub platform: Platform,

    /// Limit the number of search results (versions per package)
    #[clap(short, long)]
    pub limit: Option<usize>,

    /// Number of packages to show detailed information for
    #[clap(short = 'n', long = "limit-packages", default_value = "3")]
    pub limit_packages: usize,
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
        channels.iter().map(|c| c.name()).collect::<Vec<_>>().join(", ")
    );

    let packages = if let Some(workspace) = workspace {
        await_in_progress("searching packages...", |_| async {
            WorkspaceContext::new(CliInterface {}, workspace)
                .search(&args.package, channels, args.platform)
                .await
        })
        .await?
    } else {
        await_in_progress("searching packages...", |_| async {
            DefaultContext::new(CliInterface {})
                .search(&args.package, channels, args.platform)
                .await
        })
        .await?
    };

    // Print search results with detailed info for first N packages
    if let Err(e) = print_search_results(&packages, out, args.limit_packages, args.limit)
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
    limit_packages: usize,
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

    // Show detailed info for first N packages
    for (i, (_, records)) in by_name.iter().take(limit_packages).enumerate() {
        if i > 0 {
            writeln!(out, "\n{}", "=".repeat(60))?;
        }
        let newest = records
            .last()
            .expect("records is non-empty since packages is non-empty");
        let others: Vec<_> = records.iter().rev().skip(1).cloned().collect();
        print_package_info(newest, &others, out)?;
    }

    // Show compact table for remaining packages
    if by_name.len() > limit_packages {
        writeln!(out, "\n{}", "=".repeat(60))?;
        writeln!(
            out,
            "\nAdditional matching packages ({}):\n",
            by_name.len() - limit_packages
        )?;

        let remaining: Vec<_> = by_name
            .iter()
            .skip(limit_packages)
            .map(|(_, records)| {
                *records
                    .last()
                    .expect("records is non-empty since packages is non-empty")
            })
            .collect();
        print_matching_packages(&remaining, out, limit_versions)?;
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
        let max_displayed_versions = 4;
        let mut counter = 0;
        for (version, builds) in grouped_by_version.iter().rev() {
            writeln!(
                out,
                "{:version_width$} {:build_width$}{}",
                console::style(version.to_string()),
                console::style(builds[0].package_record.build.clone()),
                console::style(format_additional_builds_string(Some(builds[1..].to_vec()))).dim(),
                version_width = version_width,
                build_width = build_width
            )?;
            counter += 1;
            if counter == max_displayed_versions {
                writeln!(
                    out,
                    "... and {} more",
                    grouped_by_version.len() - max_displayed_versions
                )?;
                break;
            }
        }
    }

    Ok(())
}

fn print_matching_packages<W: Write>(
    packages: &[&RepoDataRecord],
    out: &mut W,
    limit: Option<usize>,
) -> io::Result<()> {
    writeln!(
        out,
        "{:40} {:19} {:19}",
        console::style("Package").bold(),
        console::style("Version").bold(),
        console::style("Channel").bold(),
    )?;

    // split off at `limit`, discard the second half
    let limit = limit.unwrap_or(usize::MAX);

    let (packages, remaining_packages) = if limit < packages.len() {
        packages.split_at(limit)
    } else {
        (packages, &[][..])
    };

    let channel_config = default_channel_config();
    for package in packages {
        // TODO: change channel fetch logic to be more robust
        // currently it relies on channel field being a url with trailing slash
        // https://github.com/conda/rattler/issues/146

        let channel_name = package
            .channel
            .as_ref()
            .and_then(|channel| Url::from_str(channel).ok())
            .and_then(|url| channel_config.strip_channel_alias(&url))
            .or_else(|| package.channel.clone())
            .unwrap_or_else(|| "<unknown>".to_string());

        let channel_name = format!("{}/{}", channel_name, package.package_record.subdir);

        let package_name = &package.package_record.name;
        let version = package.package_record.version.as_str();

        writeln!(
            out,
            "{:40} {:19} {:19}",
            console::style(package_name.as_source()).cyan().bright(),
            console::style(version),
            console::style(channel_name),
        )?;
    }

    if !remaining_packages.is_empty() {
        println!("... and {} more", remaining_packages.len());
    }

    Ok(())
}
