use std::{
    cmp::Ordering,
    collections::HashMap,
    future::{Future, IntoFuture},
    io::{self, Write},
    str::FromStr,
};

use clap::Parser;
use indexmap::IndexMap;
use itertools::Itertools;
use miette::{IntoDiagnostic, Report};
use pixi_config::{Config, default_channel_config};
use pixi_core::{WorkspaceLocator, workspace::WorkspaceLocatorError};
use pixi_progress::await_in_progress;
use pixi_utils::reqwest::build_reqwest_clients;
use rattler_conda_types::{MatchSpec, PackageName, ParseStrictness, Platform, RepoDataRecord};
use rattler_lock::Matches;
use rattler_repodata_gateway::{GatewayError, RepoData};
use regex::Regex;
use strsim::jaro;
use tracing::{debug, error};
use url::Url;

use crate::cli_config::ChannelsConfig;
use crate::cli_config::WorkspaceConfig;

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

    /// Limit the number of search results
    #[clap(short, long)]
    pub limit: Option<usize>,
}

/// fetch packages from `repo_data` using `repodata_query_func` based on
/// `filter_func`
async fn search_package_by_filter<F, QF, FR>(
    package: &PackageName,
    all_package_names: Vec<PackageName>,
    repodata_query_func: QF,
    filter_func: F,
    only_latest: bool,
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

    let mut packages: Vec<RepoDataRecord> = Vec::new();
    if only_latest {
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

            packages.extend(records_of_repo.into_values().collect_vec());
        }
        // sort all versions across all channels and platforms
        packages.sort_by(|a, b| a.package_record.version.cmp(&b.package_record.version));
    } else {
        for repo in repos {
            packages.extend(repo.into_iter().cloned().collect_vec());
        }
    }

    Ok(packages)
}

pub async fn execute_impl<W: Write>(
    args: Args,
    out: &mut W,
) -> miette::Result<Option<Vec<RepoDataRecord>>> {
    let project = match WorkspaceLocator::for_cli()
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
    let channels = args.channels.resolve_from_project(project.as_ref())?;
    eprintln!(
        "Using channels: {}",
        channels.iter().map(|c| c.name()).format(", ")
    );

    let package_name_filter = args.package;

    let project = project.as_ref();
    let client = if let Some(project) = project {
        project.authenticated_client()?.clone()
    } else {
        build_reqwest_clients(None, None)?.1
    };

    let config = Config::load_global();

    // Fetch the all names from the repodata using gateway
    let gateway = config.gateway().with_client(client).finish();

    let all_names = await_in_progress("loading all package names", |_| async {
        gateway
            .names(channels.clone(), [args.platform, Platform::NoArch])
            .await
    })
    .await
    .into_diagnostic()?;

    // Compute the repodata query function that will be used to fetch the repodata
    // for filtered package names
    let repodata_query_func = |some_specs: Vec<MatchSpec>| {
        gateway
            .query(
                channels.clone(),
                [args.platform, Platform::NoArch],
                some_specs.clone(),
            )
            .into_future()
    };

    let match_spec =
        MatchSpec::from_str(&package_name_filter, ParseStrictness::Lenient).into_diagnostic();

    let packages = if let Ok(match_spec) = match_spec {
        search_exact_package(match_spec, all_names, repodata_query_func, out).await?
    } else if package_name_filter.contains('*') {
        // If it's not a valid MatchSpec, check for wildcard
        let package_name_without_filter = package_name_filter.replace('*', "");
        let package_name = PackageName::try_from(package_name_without_filter).into_diagnostic()?;

        search_package_by_wildcard(
            package_name,
            &package_name_filter,
            all_names,
            repodata_query_func,
            args.limit,
            out,
        )
        .await?
    } else {
        return Err(miette::miette!(
            "Invalid package specification: {}",
            package_name_filter
        ));
    };

    Ok(packages)
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let mut out = io::stdout();
    execute_impl(args, &mut out).await?;
    Ok(())
}

async fn search_exact_package<W: Write, QF, FR>(
    package_spec: MatchSpec,
    all_repodata_names: Vec<PackageName>,
    repodata_query_func: QF,
    out: &mut W,
) -> miette::Result<Option<Vec<RepoDataRecord>>>
where
    QF: Fn(Vec<MatchSpec>) -> FR,
    FR: Future<Output = Result<Vec<RepoData>, GatewayError>>,
{
    let package_name_search = package_spec.name.clone().ok_or_else(|| {
        miette::miette!("could not find package name in MatchSpec {}", package_spec)
    })?;

    let packages = search_package_by_filter(
        &package_name_search,
        all_repodata_names,
        repodata_query_func,
        |pn, n| pn == n,
        false,
    )
    .await?;

    if packages.is_empty() {
        let normalized_package_name = package_name_search.as_normalized();
        return Err(miette::miette!(
            "Package {normalized_package_name} not found, please use a wildcard '*' in the search name for a broader result."
        ));
    }

    // Sort packages by version, build number and build string
    let packages = packages
        .iter()
        .filter(|&p| package_spec.matches(p))
        .sorted_by(|a, b| {
            Ord::cmp(
                &(
                    &a.package_record.version,
                    a.package_record.build_number,
                    &a.package_record.build,
                ),
                &(
                    &b.package_record.version,
                    b.package_record.build_number,
                    &b.package_record.build,
                ),
            )
        })
        .cloned()
        .collect::<Vec<RepoDataRecord>>();

    if packages.is_empty() {
        return Err(miette::miette!(
            "Package found, but MatchSpec {package_spec} does not match any record."
        ));
    }

    let newest_package = packages.last();
    if let Some(newest_package) = newest_package {
        let other_versions = packages
            .iter()
            .filter(|p| p.package_record != newest_package.package_record)
            .collect::<Vec<_>>();
        if let Err(e) = print_package_info(newest_package, &other_versions, out) {
            if e.kind() != std::io::ErrorKind::BrokenPipe {
                return Err(e).into_diagnostic();
            }
        }
    }

    Ok(newest_package.map(|package| vec![package.clone()]))
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

    if let Some(run_exports) = package.package_record.run_exports.as_ref() {
        writeln!(out, "\nRun exports:")?;
        let mut print_run_exports = |name: &str, run_exports: &[String]| {
            if !run_exports.is_empty() {
                writeln!(out, "  {name}:")?;
                for run_export in run_exports {
                    writeln!(out, "   - {}", run_export)?;
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

async fn search_package_by_wildcard<W: Write, QF, FR>(
    package_name: PackageName,
    package_name_filter: &str,
    all_package_names: Vec<PackageName>,
    repodata_query_func: QF,
    limit: Option<usize>,
    out: &mut W,
) -> miette::Result<Option<Vec<RepoDataRecord>>>
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
            true,
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
            true,
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

    Ok(Some(packages))
}

fn print_matching_packages<W: Write>(
    packages: &[RepoDataRecord],
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
