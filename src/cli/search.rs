use std::{cmp::Ordering, path::PathBuf};

use clap::Parser;
use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::{Channel, ChannelConfig, Platform, RepoDataRecord};
use rattler_repodata_gateway::sparse::SparseRepoData;
use strsim::jaro;
use tokio::task::spawn_blocking;

use crate::{progress::await_in_progress, repodata::fetch_sparse_repodata, Project};

/// Search a package, output will list the latest version of package
#[derive(Debug, Parser)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    /// Name of package to search
    #[arg(required = true)]
    pub package: String,

    /// Channel to specifically search package
    #[clap(short, long, default_values = ["conda-forge"])]
    channel: Vec<String>,

    /// The path to 'pixi.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Limit the number of search results
    #[clap(short, long, default_value_t = 15)]
    limit: usize,
}

/// fetch packages from `repo_data` based on `filter_func`
fn search_package_by_filter<F>(
    package: &str,
    repo_data: &[SparseRepoData],
    filter_func: F,
) -> miette::Result<Vec<RepoDataRecord>>
where
    F: Fn(&str, &str) -> bool,
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
            let mut records = repo.load_records(package).into_diagnostic()?;
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
    let project = Project::load_or_else_discover(args.manifest_path.as_deref()).ok();

    let channels = if let Some(project) = project {
        project.channels().to_owned()
    } else {
        let channel_config = ChannelConfig::default();
        args.channel
            .iter()
            .map(|c| Channel::from_str(c, &channel_config))
            .collect::<Result<Vec<Channel>, _>>()
            .into_diagnostic()?
    };

    let limit = args.limit;
    let package_name = args.package;
    let platforms = [Platform::current()];
    let repo_data = fetch_sparse_repodata(&channels, &platforms).await?;

    let p = package_name.clone();
    let mut packages = await_in_progress(
        "searching packages",
        spawn_blocking(move || {
            let packages = search_package_by_filter(&p, &repo_data, |pn, n| pn.contains(n));
            match packages {
                Ok(packages) => {
                    if packages.is_empty() {
                        let similarity = 0.6;
                        return search_package_by_filter(&p, &repo_data, |pn, n| {
                            jaro(pn, n) > similarity
                        });
                    }
                    Ok(packages)
                }
                Err(e) => Err(e),
            }
        }),
    )
    .await
    .into_diagnostic()??;

    packages.sort_by(|a, b| {
        let ord = jaro(&b.package_record.name, &package_name)
            .partial_cmp(&jaro(&a.package_record.name, &package_name));
        if let Some(ord) = ord {
            ord
        } else {
            Ordering::Equal
        }
    });

    if packages.is_empty() {
        return Err(miette::miette!("Could not find {package_name}"));
    }

    // split off at `limit`, discard the second half
    if packages.len() > limit {
        let _ = packages.split_off(limit);
    }

    println!(
        "{:40} {:19} {:19}",
        console::style("Package").bold(),
        console::style("Version").bold(),
        console::style("Channel").bold(),
    );
    for package in packages {
        // TODO: change channel fetch logic to be more robust
        // currently it relies on channel field being a url with trailing slash
        // https://github.com/mamba-org/rattler/issues/146
        let channel = package.channel.split('/').collect::<Vec<_>>();
        let channel_name = channel[channel.len() - 2];

        let package_name = package.package_record.name;
        let version = package.package_record.version.as_str();

        println!(
            "{:40} {:19} {:19}",
            console::style(package_name).cyan().bright(),
            console::style(version),
            console::style(channel_name),
        );
    }

    Ok(())
}
