use indexmap::IndexSet;
use miette::IntoDiagnostic;
use pixi_config::Config;
use pixi_core::Workspace;
use pixi_utils::reqwest::build_lazy_reqwest_clients;
use rattler_conda_types::{
    Channel, MatchSpec, PackageName, PackageNameMatcher, Platform, RepoDataRecord,
};

/// Search for packages matching `matchspec`. A bare package name without an
/// exact match falls back to a "contains" match over the package names,
/// ranked prefix-first.
///
/// `fuzzy_limit` caps how many fuzzy-matched packages get their records
/// fetched; on sharded channels every package is a separate HTTP request.
/// `None` fetches everything.
pub async fn search(
    workspace: Option<&Workspace>,
    config: Config,
    matchspec: MatchSpec,
    channels: IndexSet<Channel>,
    platforms: Vec<Platform>,
    fuzzy_limit: Option<usize>,
) -> miette::Result<Vec<RepoDataRecord>> {
    let client = if let Some(workspace) = workspace {
        workspace.authenticated_client()?.clone()
    } else {
        build_lazy_reqwest_clients(Some(&config), None)?.1
    };

    let gateway = config.gateway().with_client(client).finish();

    let run_query = |specs: Vec<MatchSpec>| {
        let gateway = &gateway;
        let channels = channels.clone();
        let platforms = platforms.clone();
        async move {
            let repo_data = gateway
                .query(channels, platforms, specs)
                .recursive(false)
                .await
                .into_diagnostic()?;

            let mut packages: Vec<RepoDataRecord> = Vec::new();
            for repo in repo_data {
                packages.extend(repo.iter().cloned());
            }
            Ok::<Vec<RepoDataRecord>, miette::Report>(packages)
        }
    };

    let mut packages = run_query(vec![matchspec.clone()]).await?;

    if packages.is_empty()
        && let Some(name) = bare_exact_name(&matchspec)
    {
        // The subdirs are already cached in the gateway, so listing the
        // package names is cheap: no record downloads.
        let all_names = gateway
            .names(channels.clone(), platforms.clone())
            .execute()
            .await
            .into_diagnostic()?;

        let needle = name.as_normalized();
        let mut matches: Vec<&PackageName> = all_names
            .iter()
            .filter(|n| n.as_normalized().contains(needle))
            .collect();
        matches.sort_by_key(|n| {
            (
                !n.as_normalized().starts_with(needle),
                n.as_normalized().to_string(),
            )
        });
        matches.truncate(fuzzy_limit.unwrap_or(usize::MAX));

        let specs: Vec<MatchSpec> = matches
            .into_iter()
            .map(|n| MatchSpec {
                name: PackageNameMatcher::Exact(n.clone()),
                ..MatchSpec::default()
            })
            .collect();

        if !specs.is_empty() {
            packages = run_query(specs).await?;
            // Prefix matches first, then natural (name, version) ordering.
            let needle = needle.to_string();
            packages.sort_by(|a, b| {
                let a_prefix = a.package_record.name.as_normalized().starts_with(&needle);
                let b_prefix = b.package_record.name.as_normalized().starts_with(&needle);
                b_prefix.cmp(&a_prefix).then_with(|| a.cmp(b))
            });
            if !packages.is_empty() {
                return Ok(packages);
            }
        }
    }

    if packages.is_empty() {
        return Err(no_packages_found(&matchspec));
    }

    packages.sort();

    Ok(packages)
}

fn no_packages_found(matchspec: &MatchSpec) -> miette::Report {
    miette::miette!(
        help = "Try glob patterns like 'python*' or '*numpy*'",
        "No packages found matching '{}'",
        matchspec
    )
}

/// Returns the package name if the match spec is nothing more than a bare,
/// exact package name — the only case where broadening into a fuzzy search
/// is safe.
fn bare_exact_name(spec: &MatchSpec) -> Option<&PackageName> {
    // Destructured without `..` so a new MatchSpec field is a compile error
    // here, forcing a decision on whether it keeps the spec "bare".
    match spec {
        MatchSpec {
            name,
            version: None,
            build: None,
            build_number: None,
            file_name: None,
            extras: None,
            flags: None,
            channel: None,
            subdir: None,
            namespace: None,
            md5: None,
            sha256: None,
            url: None,
            license: None,
            license_family: None,
            condition: None,
            track_features: None,
        } => name.as_exact(),
        _ => None,
    }
}
