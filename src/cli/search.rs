use std::borrow::Cow;
use std::io::{self, Write};
use std::{cmp::Ordering, path::PathBuf};

use clap::Parser;
use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::{Channel, ChannelConfig, PackageName, Platform, RepoDataRecord};
use rattler_repodata_gateway::sparse::SparseRepoData;
use regex::Regex;

use strsim::jaro;
use tokio::task::spawn_blocking;

use crate::auth::make_client;
use crate::{progress::await_in_progress, repodata::fetch_sparse_repodata, Project};

/// Search a package, output will list the latest version of package
#[derive(Debug, Parser)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    /// Name of package to search
    #[arg(required = true)]
    pub package: String,

    /// Channel to specifically search package, defaults to
    /// project channels or conda-forge
    #[clap(short, long)]
    channel: Option<Vec<String>>,

    /// The path to 'pixi.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Limit the number of search results
    #[clap(short, long, default_value_t = 15)]
    limit: usize,
}

/// fetch packages from `repo_data` based on `filter_func`
fn search_package_by_filter<F>(
    package: &PackageName,
    repo_data: &[SparseRepoData],
    filter_func: F,
) -> miette::Result<Vec<RepoDataRecord>>
where
    F: Fn(&str, &PackageName) -> bool,
{
    let similar_packages = repo_data
        .iter()
        .flat_map(|repo| {
            repo.package_names()
                .filter(|&name| filter_func(name, package))
        })
        .collect::<Vec<&str>>();

    let mut latest_packages = Vec::new();

    // search for `similar_packages` in all platform's repodata
    // add the latest version of the fetched package to latest_packages vector
    for repo in repo_data {
        for package in &similar_packages {
            let mut records = repo
                .load_records(&PackageName::new_unchecked(*package))
                .into_diagnostic()?;
            // sort records by version, get the latest one
            records.sort_by(|a, b| a.package_record.version.cmp(&b.package_record.version));
            let latest_package = records.last().cloned();
            if let Some(latest_package) = latest_package {
                latest_packages.push(latest_package);
            }
        }
    }

    latest_packages = latest_packages
        .into_iter()
        .unique_by(|record| record.package_record.name.clone())
        .collect::<Vec<_>>();

    Ok(latest_packages)
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let stdout = io::stdout();
    let project = Project::load_or_else_discover(args.manifest_path.as_deref()).ok();

    let channel_config = ChannelConfig::default();

    let channels = match (args.channel, project.as_ref()) {
        // if user passes channels through the channel flag
        (Some(c), _) => c
            .iter()
            .map(|c| Channel::from_str(c, &channel_config))
            .map_ok(Cow::Owned)
            .collect::<Result<Vec<_>, _>>()
            .into_diagnostic()?,
        // if user doesn't pass channels and we are in a project
        (None, Some(p)) => p.channels().into_iter().map(Cow::Borrowed).collect(),
        // if user doesn't pass channels and we are not in project
        (None, None) => vec![Cow::Owned(
            Channel::from_str("conda-forge", &channel_config).into_diagnostic()?,
        )],
    };

    let package_name_filter = args.package;
    let authenticated_client = make_client();
    let repo_data = fetch_sparse_repodata(
        channels.iter().map(AsRef::as_ref),
        [Platform::current()],
        &authenticated_client,
    )
    .await?;

    // When package name filter contains * (wildcard), it will search and display a list of packages matching this filter
    if package_name_filter.contains('*') {
        let package_name_without_filter = package_name_filter.replace('*', "");
        let package_name = PackageName::try_from(package_name_without_filter).into_diagnostic()?;

        let limit = args.limit;

        search_package_by_wildcard(package_name, &package_name_filter, repo_data, limit, stdout)
            .await?;
    }
    // If package name filter doesn't contain * (wildcard), it will search and display specific package info (if any package is found)
    else {
        let package_name = PackageName::try_from(package_name_filter).into_diagnostic()?;
        search_exact_package(package_name, repo_data, stdout).await?;
    }

    Ok(())
}

async fn search_exact_package<W: Write>(
    package_name: PackageName,
    repo_data: Vec<SparseRepoData>,
    out: W,
) -> miette::Result<()> {
    let package_name_search = package_name.clone();
    let packages = await_in_progress(
        "searching packages",
        spawn_blocking(move || {
            search_package_by_filter(&package_name_search, &repo_data, |pn, n| {
                pn == n.as_normalized()
            })
        }),
    )
    .await
    .into_diagnostic()??;

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

async fn search_package_by_wildcard<W: Write>(
    package_name: PackageName,
    package_name_filter: &str,
    repo_data: Vec<SparseRepoData>,
    limit: usize,
    out: W,
) -> miette::Result<()> {
    let wildcard_pattern = Regex::new(&format!("^{}$", &package_name_filter.replace('*', ".*")))
        .expect("Expect only characters and/or * (wildcard).");

    let package_name_search = package_name.clone();
    let mut packages = await_in_progress(
        "searching packages",
        spawn_blocking(move || {
            let packages = search_package_by_filter(&package_name_search, &repo_data, |pn, _| {
                wildcard_pattern.is_match(pn)
            });
            match packages {
                Ok(packages) => {
                    if packages.is_empty() {
                        let similarity = 0.6;
                        return search_package_by_filter(
                            &package_name_search,
                            &repo_data,
                            |pn, n| jaro(pn, n.as_normalized()) > similarity,
                        );
                    }
                    Ok(packages)
                }
                Err(e) => Err(e),
            }
        }),
    )
    .await
    .into_diagnostic()??;

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

    // split off at `limit`, discard the second half
    if packages.len() > limit {
        let _ = packages.split_off(limit);
    }

    if let Err(e) = print_matching_packages(packages, out) {
        if e.kind() != std::io::ErrorKind::BrokenPipe {
            return Err(e).into_diagnostic();
        }
    }

    Ok(())
}

fn print_matching_packages<W: Write>(packages: Vec<RepoDataRecord>, mut out: W) -> io::Result<()> {
    writeln!(
        out,
        "{:40} {:19} {:19}",
        console::style("Package").bold(),
        console::style("Version").bold(),
        console::style("Channel").bold(),
    )?;

    for package in packages {
        // TODO: change channel fetch logic to be more robust
        // currently it relies on channel field being a url with trailing slash
        // https://github.com/mamba-org/rattler/issues/146
        let channel = package.channel.split('/').collect::<Vec<_>>();
        let channel_name = channel[channel.len() - 2];

        let package_name = package.package_record.name;
        let version = package.package_record.version.as_str();

        writeln!(
            out,
            "{:40} {:19} {:19}",
            console::style(package_name.as_source()).cyan().bright(),
            console::style(version),
            console::style(channel_name),
        )?;
    }

    Ok(())
}
