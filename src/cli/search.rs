use std::cmp::Ordering;
use std::collections::HashMap;
use std::future::{Future, IntoFuture};
use std::io::{self, Write};
use std::str::FromStr;

use clap::Parser;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_config::default_channel_config;
use pixi_progress::await_in_progress;
use pixi_utils::reqwest::build_reqwest_clients;
use rattler_conda_types::MatchSpec;
use rattler_conda_types::{PackageName, Platform, RepoDataRecord};
use rattler_repodata_gateway::{GatewayError, RepoData};
use regex::Regex;
use strsim::jaro;
use url::Url;

use crate::cli::cli_config::ProjectConfig;
use crate::Project;
use pixi_config::Config;

use super::cli_config::ChannelsConfig;

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
    channels: ChannelsConfig,

    #[clap(flatten)]
    pub project_config: ProjectConfig,

    /// The platform to search for, defaults to current platform
    #[arg(short, long, default_value_t = Platform::current())]
    pub platform: Platform,

    /// Limit the number of search results
    #[clap(short, long)]
    limit: Option<usize>,
}

/// fetch packages from `repo_data` using `repodata_query_func` based on `filter_func`
async fn search_package_by_filter<F, QF, FR>(
    package: &PackageName,
    all_package_names: Vec<PackageName>,
    repodata_query_func: QF,
    filter_func: F,
) -> miette::Result<Vec<RepoDataRecord>>
where
    F: Fn(&PackageName, &PackageName) -> bool,
    QF: Fn(Vec<MatchSpec>) -> FR,
    FR: Future<Output = Result<Vec<RepoData>, GatewayError>>,
{
    let similar_packages = all_package_names
        .iter()
        .filter(|&name| filter_func(name, package))
        .cloned()
        .collect_vec();

    // Transform the package names into `MatchSpec`s

    let specs = similar_packages
        .iter()
        .cloned()
        .map(MatchSpec::from)
        .collect();

    let repos: Vec<RepoData> = repodata_query_func(specs).await.into_diagnostic()?;

    let mut latest_packages: Vec<RepoDataRecord> = Vec::new();

    for repo in repos {
        // sort records by version, get the latest one of each package
        let records_of_repo: HashMap<String, RepoDataRecord> = repo
            .into_iter()
            .sorted_by(|a, b| a.package_record.version.cmp(&b.package_record.version))
            .map(|record| {
                (
                    record.package_record.name.as_normalized().to_string(),
                    record.clone(),
                )
            })
            .collect();

        latest_packages.extend(records_of_repo.into_values().collect_vec());
    }

    Ok(latest_packages)
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let stdout = io::stdout();
    let project = Project::load_or_else_discover(args.project_config.manifest_path.as_deref()).ok();

    // Resolve channels from project / CLI args
    let channels = args.channels.resolve_from_project(project.as_ref());
    eprintln!(
        "Using channels: {}",
        channels.iter().map(|c| c.name()).format(", ")
    );

    let package_name_filter = args.package;

    let client = project
        .as_ref()
        .map(|p| p.authenticated_client().clone())
        .unwrap_or_else(|| build_reqwest_clients(None).1);

    let config = Config::load_global();

    // Fetch the all names from the repodata using gateway
    let gateway = config.gateway(client.clone());

    let all_names = await_in_progress("loading all package names", |_| async {
        gateway
            .names(channels.clone(), [args.platform, Platform::NoArch])
            .await
    })
    .await
    .into_diagnostic()?;

    // Compute the repodata query function that will be used to fetch the repodata for
    // filtered package names

    let repodata_query_func = |some_specs: Vec<MatchSpec>| {
        gateway
            .query(
                channels.clone(),
                [args.platform, Platform::NoArch],
                some_specs.clone(),
            )
            .into_future()
    };

    // When package name filter contains * (wildcard), it will search and display a
    // list of packages matching this filter
    if package_name_filter.contains('*') {
        let package_name_without_filter = package_name_filter.replace('*', "");
        let package_name = PackageName::try_from(package_name_without_filter).into_diagnostic()?;

        search_package_by_wildcard(
            package_name,
            &package_name_filter,
            all_names,
            repodata_query_func,
            args.limit,
            stdout,
        )
        .await?;
    }
    // If package name filter doesn't contain * (wildcard), it will search and display specific
    // package info (if any package is found)
    else {
        let package_name = PackageName::try_from(package_name_filter).into_diagnostic()?;

        search_exact_package(package_name, all_names, repodata_query_func, stdout).await?;
    }

    Project::warn_on_discovered_from_env(args.project_config.manifest_path.as_deref());
    Ok(())
}

async fn search_exact_package<W: Write, QF, FR>(
    package_name: PackageName,
    all_repodata_names: Vec<PackageName>,
    repodata_query_func: QF,
    out: W,
) -> miette::Result<()>
where
    QF: Fn(Vec<MatchSpec>) -> FR,
    FR: Future<Output = Result<Vec<RepoData>, GatewayError>>,
{
    let package_name_search = package_name.clone();
    let packages = search_package_by_filter(
        &package_name_search,
        all_repodata_names,
        repodata_query_func,
        |pn, n| pn == n,
    )
    .await?;

    if packages.is_empty() {
        let normalized_package_name = package_name.as_normalized();
        return Err(miette::miette!("Package {normalized_package_name} not found, please use a wildcard '*' in the search name for a broader result."));
    }

    let package = packages.last();
    if let Some(package) = package {
        if let Err(e) = print_package_info(package, out) {
            if e.kind() != std::io::ErrorKind::BrokenPipe {
                return Err(e).into_diagnostic();
            }
        }
    }

    Ok(())
}

fn print_package_info<W: Write>(package: &RepoDataRecord, mut out: W) -> io::Result<()> {
    writeln!(out)?;

    let package = package.clone();
    let package_name = package.package_record.name.as_source();
    let build = &package.package_record.build;
    let package_info = format!("{} {}", console::style(package_name), console::style(build));
    writeln!(out, "{}", package_info)?;
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
        console::style(package.file_name)
    )?;

    writeln!(
        out,
        "{:19} {:19}",
        console::style("URL"),
        console::style(package.url)
    )?;

    let md5 = match package.package_record.md5 {
        Some(md5) => format!("{:x}", md5),
        None => "Not available".to_string(),
    };
    writeln!(
        out,
        "{:19} {:19}",
        console::style("MD5"),
        console::style(md5)
    )?;

    let sha256 = match package.package_record.sha256 {
        Some(sha256) => format!("{:x}", sha256),
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
        writeln!(out, " - {}", dependency)?;
    }

    Ok(())
}

async fn search_package_by_wildcard<W: Write, QF, FR>(
    package_name: PackageName,
    package_name_filter: &str,
    all_package_names: Vec<PackageName>,
    repodata_query_func: QF,
    limit: Option<usize>,
    out: W,
) -> miette::Result<()>
where
    QF: Fn(Vec<MatchSpec>) -> FR + Clone,
    FR: Future<Output = Result<Vec<RepoData>, GatewayError>>,
{
    let wildcard_pattern = Regex::new(&format!("^{}$", &package_name_filter.replace('*', ".*")))
        .expect("Expect only characters and/or * (wildcard).");

    let package_name_search = package_name.clone();

    let mut packages = await_in_progress("searching packages", |_| async {
        let packages = search_package_by_filter(
            &package_name_search,
            all_package_names.clone(),
            repodata_query_func.clone(),
            |pn, _| wildcard_pattern.is_match(pn.as_normalized()),
        )
        .await?;

        if !packages.is_empty() {
            return Ok(packages);
        }

        tracing::info!("No packages found with wildcard search, trying with fuzzy search.");
        let similarity = 0.85;
        search_package_by_filter(
            &package_name_search,
            all_package_names,
            repodata_query_func,
            |pn, n| jaro(pn.as_normalized(), n.as_normalized()) > similarity,
        )
        .await
    })
    .await?;

    let normalized_package_name = package_name.as_normalized();
    packages.sort_by(|a, b| {
        let ord = jaro(
            b.package_record.name.as_normalized(),
            normalized_package_name,
        )
        .partial_cmp(&jaro(
            a.package_record.name.as_normalized(),
            normalized_package_name,
        ));
        if let Some(ord) = ord {
            ord
        } else {
            Ordering::Equal
        }
    });

    if packages.is_empty() {
        return Err(miette::miette!("Could not find {normalized_package_name}"));
    }

    if let Err(e) = print_matching_packages(&packages, out, limit) {
        if e.kind() != std::io::ErrorKind::BrokenPipe {
            return Err(e).into_diagnostic();
        }
    }

    Ok(())
}

fn print_matching_packages<W: Write>(
    packages: &[RepoDataRecord],
    mut out: W,
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
        // https://github.com/mamba-org/rattler/issues/146

        let channel_name = Url::from_str(&package.channel)
            .ok()
            .and_then(|url| channel_config.strip_channel_alias(&url))
            .unwrap_or_else(|| package.channel.to_string());

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
