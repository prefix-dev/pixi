use std::{cmp::Ordering, collections::HashMap, future::Future};

use indexmap::IndexSet;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_config::Config;
use pixi_core::Workspace;
use pixi_utils::reqwest::build_lazy_reqwest_clients;
use rattler_conda_types::{Channel, MatchSpec, PackageName, Platform, RepoDataRecord};
use rattler_lock::Matches;
use rattler_repodata_gateway::{GatewayError, RepoData};
use regex::Regex;
use strsim::jaro;

use crate::Interface;

pub async fn search_exact<I: Interface>(
    _interface: &I,
    workspace: Option<&Workspace>,
    match_spec: MatchSpec,
    channels: IndexSet<Channel>,
    platform: Platform,
) -> miette::Result<Option<Vec<RepoDataRecord>>> {
    let client = if let Some(workspace) = workspace {
        workspace.authenticated_client()?.clone()
    } else {
        build_lazy_reqwest_clients(None, None)?.1
    };

    let config = Config::load_global();

    // Fetch the all names from the repodata using gateway
    let gateway = config.gateway().with_client(client).finish();

    let all_package_names = gateway
        .names(channels.clone(), [platform, Platform::NoArch])
        .await
        .into_diagnostic()?;

    // Compute the repodata query function that will be used to fetch the repodata
    // for filtered package names
    let repodata_query_func = |some_specs: Vec<MatchSpec>| {
        gateway
            .query(
                channels.clone(),
                [platform, Platform::NoArch],
                some_specs.clone(),
            )
            .into_future()
    };

    let package_name_search = match_spec.name.clone().ok_or_else(|| {
        miette::miette!("could not find package name in MatchSpec {}", match_spec)
    })?;

    let package_name = package_name_search
        .as_exact()
        .ok_or_else(|| miette::miette!("search does not support wildcard package names"))?;

    let packages = search_package_by_filter(
        package_name,
        all_package_names,
        repodata_query_func,
        |pn, n| pn == n,
        false,
    )
    .await?;

    if packages.is_empty() {
        let normalized_package_name = package_name.as_normalized();
        return Err(miette::miette!(
            "Package {normalized_package_name} not found, please use a wildcard '*' in the search name for a broader result."
        ));
    }

    // Sort packages by version, build number and build string
    let packages = packages
        .iter()
        .filter(|&p| match_spec.matches(p))
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
            "Package found, but MatchSpec {match_spec} does not match any record."
        ));
    }

    Ok(Some(packages))
}

pub async fn search_wildcard<I: Interface>(
    _interface: &I,
    workspace: Option<&Workspace>,
    package_name_filter: &str,
    channels: IndexSet<Channel>,
    platform: Platform,
) -> miette::Result<Option<Vec<RepoDataRecord>>> {
    let client = if let Some(workspace) = workspace {
        workspace.authenticated_client()?.clone()
    } else {
        build_lazy_reqwest_clients(None, None)?.1
    };

    let config = Config::load_global();

    // Fetch the all names from the repodata using gateway
    let gateway = config.gateway().with_client(client).finish();

    let all_package_names = gateway
        .names(channels.clone(), [platform, Platform::NoArch])
        .await
        .into_diagnostic()?;

    // Compute the repodata query function that will be used to fetch the repodata
    // for filtered package names
    let repodata_query_func = |some_specs: Vec<MatchSpec>| {
        gateway
            .query(
                channels.clone(),
                [platform, Platform::NoArch],
                some_specs.clone(),
            )
            .into_future()
    };

    let package_name_without_filter = package_name_filter.replace('*', "");
    let package_name = PackageName::try_from(package_name_without_filter).into_diagnostic()?;

    let wildcard_pattern = Regex::new(&format!("^{}$", &package_name_filter.replace('*', ".*")))
        .expect("Expect only characters and/or * (wildcard).");

    let package_name_search = package_name.clone();

    let mut packages = search_package_by_filter(
        &package_name_search,
        all_package_names.clone(),
        repodata_query_func,
        |pn, _| wildcard_pattern.is_match(pn.as_normalized()),
        true,
    )
    .await?;

    if packages.is_empty() {
        tracing::info!("No packages found with wildcard search, trying with fuzzy search.");
        let similarity = 0.85;
        packages = search_package_by_filter(
            &package_name_search,
            all_package_names,
            repodata_query_func,
            |pn, n| jaro(pn.as_normalized(), n.as_normalized()) > similarity,
            true,
        )
        .await?;
    }

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

    Ok(Some(packages))
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
