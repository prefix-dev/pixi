use std::cmp::Ordering;
use std::collections::HashMap;
use std::future::{Future, IntoFuture};
use std::io::{self, Write};
use std::str::FromStr;

use clap::Parser;
use ignore::gitignore::Glob;
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
use tabled::settings::style::{HorizontalLine, VerticalLine};
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
    pub search_spec: String,

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

use tabled::{builder::Builder, settings::Style};

/// Print a beautiful table of repodata records using tabled
fn print_table(records: &[RepoDataRecord], group_by_version: bool) {
    let mut builder = Builder::default();

    let style = Style::modern()
        .horizontals([(1, HorizontalLine::inherit(Style::modern()).horizontal('═'))])
        .remove_frame()
        .remove_horizontal()
        .remove_vertical();

    // header line
    builder.push_record(vec!["Name", "Version", "Build", "Channel", "Subdir"]);

    if group_by_version {
        // Group records by version
        let mut version_groups: HashMap<String, Vec<&RepoDataRecord>> = HashMap::new();
        for record in records {
            version_groups
                .entry(record.package_record.version.to_string())
                .or_default()
                .push(record);
        }

        for (version, records) in version_groups
            .iter()
            .sorted_by(|a, b| a.0.cmp(b.0))
        {
            // Sort records within version group
            let mut records = records.to_vec();
            records.sort_by(|a, b| a.package_record.build.cmp(&b.package_record.build));

            // Take first record to display version info
            let first = records[0];
            let build_count = if records.len() > 1 {
                format!("{} (+{})", first.package_record.build, records.len() - 1)
            } else {
                first.package_record.build.to_string()
            };

            let row = vec![
                first.package_record.name.as_normalized().to_string(),
                version.to_string(),
                build_count,
                first.channel.to_string(),
                first.package_record.subdir.to_string(),
            ];
            builder.push_record(row);
        }
    } else {
        // Original non-grouped display
        for record in records
            .iter()
            .sorted_by(|a, b| a.package_record.version.cmp(&b.package_record.version))
        {
            let row = vec![
                record.package_record.name.as_normalized().to_string(),
                record.package_record.version.to_string(),
                record.package_record.build.to_string(),
                record.channel.to_string(),
                record.package_record.subdir.to_string(),
            ];
            builder.push_record(row);
        }
    }

    let mut table = builder.build();
    println!("{}", table.with(style));
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.project_config.manifest_path.as_deref()).ok();

    // Resolve channels from project / CLI args
    let channels = args.channels.resolve_from_project(project.as_ref())?;
    eprintln!(
        "Using channels: {}",
        channels.iter().map(|c| c.name()).format(", ")
    );

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

    let matched_names = match_names(&all_names, &args.search_spec);
    println!("matched_names: {:?}", matched_names);

    // Compute the repodata query function that will be used to fetch the repodata for
    // filtered package names
    let repodata_query_func = |specs: Vec<MatchSpec>| {
        gateway
            .query(channels.clone(), [args.platform, Platform::NoArch], specs)
            .into_future()
    };

    for name in matched_names {
        let result = repodata_query_func(vec![&name])
            .await
            .unwrap();

            // flatten the records
        let mut flattened = Vec::new();
        for repo in result {
            flattened.extend(repo.into_iter().cloned());
        }
        print_table(flattened.as_slice(), true);

        Project::warn_on_discovered_from_env(args.project_config.manifest_path.as_deref());
    }
    Ok(())
}

// Use the `glob` crate to match the search_spec against the all_names
fn match_names(all_names: &[PackageName], search_spec: &str) -> Vec<PackageName> {
    let glob = globset::Glob::from_str(search_spec).unwrap().compile_matcher();
    all_names.iter().filter(|name| glob.is_match(name.as_normalized())).cloned().collect()
}
