use std::cmp::Ordering;

use clap::Parser;
use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::{Channel, ChannelConfig, Platform, RepoDataRecord};
use rattler_repodata_gateway::sparse::SparseRepoData;
use strsim::jaro;
use tokio::task::spawn_blocking;

use crate::{progress::await_in_progress, repodata::fetch_sparse_repodata};

#[derive(Debug, Parser)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    #[arg(required = true)]
    pub package: String,

    #[clap(short, long, default_values = ["conda-forge"])]
    channel: Vec<String>,
}

/// fetch packages with names similar to the queried package from the `&[SparseRepoData]`
/// provided given a similarity between 0.0 to 1.0
fn search_package_by_similarity(
    package: &str,
    repo_data: &[SparseRepoData],
    similarity: f64,
) -> miette::Result<Vec<RepoDataRecord>> {
    let similar_packages = repo_data
        .iter()
        .flat_map(|repo| {
            repo.package_names()
                .filter(|&name| jaro(name, package) > similarity)
        })
        .collect::<Vec<&str>>();

    let mut latest_packages = Vec::new();

    for repo in repo_data {
        for package in &similar_packages {
            let mut records = repo.load_records(package).into_diagnostic()?;
            records.sort_by(|a, b| a.package_record.version.cmp(&b.package_record.version));
            let latest_package = records.last().cloned();
            if let Some(latest_package) = latest_package {
                latest_packages.push(latest_package);
            }
        }
    }

    latest_packages.sort_by(|a, b| {
        let ord = jaro(&a.package_record.name, package)
            .partial_cmp(&jaro(&b.package_record.name, package));
        if let Some(ord) = ord {
            ord
        } else {
            Ordering::Equal
        }
    });

    latest_packages = latest_packages
        .into_iter()
        .unique_by(|record| record.package_record.name.clone())
        .collect::<Vec<_>>();

    Ok(latest_packages)
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let channel_config = ChannelConfig::default();
    let channels = args
        .channel
        .iter()
        .map(|c| Channel::from_str(c, &channel_config))
        .collect::<Result<Vec<Channel>, _>>()
        .into_diagnostic()?;

    let similarity = 0.8;
    let package_name = args.package;
    let platforms = [Platform::current()];
    let repo_data = fetch_sparse_repodata(&channels, &platforms).await?;

    let p = package_name.clone();
    let packages = await_in_progress(
        "searching packages",
        spawn_blocking(move || search_package_by_similarity(&p, &repo_data, similarity)),
    )
    .await
    .into_diagnostic()??;

    if packages.is_empty() {
        // don't know if this is the best way to do it
        return Err(miette::miette!("Could not find {package_name}"));
    }

    for package in packages {
        // TODO: change channel fetch logic to be more robust
        // currently it relies on channel field being a url with trailing slash
        // https://github.com/mamba-org/rattler/issues/146
        let channel = package.channel.split('/').collect::<Vec<_>>();
        let channel_name = channel[channel.len() - 2];

        let package_name = package.package_record.name;
        let version = package.package_record.version.as_str();

        // TODO: prettify stdout
        println!("{channel_name}/{package_name}: {version}");
    }

    Ok(())
}
