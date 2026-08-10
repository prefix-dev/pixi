use indexmap::IndexSet;
use miette::IntoDiagnostic;
use pixi_config::Config;
use pixi_core::Workspace;
use pixi_utils::reqwest::build_lazy_reqwest_clients;
use rattler_conda_types::{
    Channel, MatchSpec, PackageName, PackageNameMatcher, Platform, RepoDataRecord,
};

/// Search for packages matching `matchspec`.
///
/// For a bare package name the package-name indexes are resolved first (cheap:
/// shard index / repodata keys only, no record downloads). If the name exists
/// only that package's records are fetched; otherwise the search falls back to
/// fuzzy matching: a "contains" match over the names, ranked prefix-first.
///
/// `fuzzy_limit` caps how many fuzzy-matched packages get their records
/// fetched. This matters on sharded channels where every package's records are
/// a separate HTTP request: a broad term like `ros` matches over a thousand
/// names, and fetching records for all of them downloads hundreds of shards
/// while only a handful of packages are displayed. `None` fetches everything.
pub async fn search(
    workspace: Option<&Workspace>,
    matchspec: MatchSpec,
    channels: IndexSet<Channel>,
    platforms: Vec<Platform>,
    fuzzy_limit: Option<usize>,
) -> miette::Result<Vec<RepoDataRecord>> {
    let client = if let Some(workspace) = workspace {
        workspace.authenticated_client()?.clone()
    } else {
        build_lazy_reqwest_clients(None, None)?.1
    };

    let config = Config::load_global();
    let gateway = config.gateway().with_client(client).finish();

    let bare_name = bare_exact_name(&matchspec);

    let specs: Vec<MatchSpec> = if let Some(name) = bare_name {
        let all_names = gateway
            .names(channels.clone(), platforms.clone())
            .execute()
            .await
            .into_diagnostic()?;

        let needle = name.as_normalized();
        let exact_exists = all_names.iter().any(|n| n.as_normalized() == needle);

        let selected: Vec<PackageName> = if exact_exists {
            vec![name.clone()]
        } else {
            // Fuzzy fallback: contains-match, ranked prefix-first then
            // alphabetical, capped so we don't fetch records for packages
            // that won't be shown anyway.
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
            matches.into_iter().cloned().collect()
        };

        if selected.is_empty() {
            return Err(no_packages_found(&matchspec));
        }

        selected
            .into_iter()
            .map(|n| MatchSpec {
                name: PackageNameMatcher::Exact(n),
                ..MatchSpec::default()
            })
            .collect()
    } else {
        // Globs or extra constraints: respect the user's input as-is.
        vec![matchspec.clone()]
    };

    let repo_data = gateway
        .query(channels, platforms, specs)
        .recursive(false)
        .await
        .into_diagnostic()?;

    let mut packages: Vec<RepoDataRecord> = Vec::new();
    for repo in repo_data {
        packages.extend(repo.iter().cloned());
    }

    if packages.is_empty() {
        return Err(no_packages_found(&matchspec));
    }

    // Rank prefix matches first so the closest names are shown on top, then
    // fall back to the natural (name, version) ordering.
    if let Some(name) = bare_name {
        let needle = name.as_normalized().to_string();
        packages.sort_by(|a, b| {
            let a_prefix = a.package_record.name.as_normalized().starts_with(&needle);
            let b_prefix = b.package_record.name.as_normalized().starts_with(&needle);
            b_prefix.cmp(&a_prefix).then_with(|| a.cmp(b))
        });
    } else {
        packages.sort();
    }

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
/// exact package name (no glob, version, build or any other constraint).
///
/// This is used to decide whether it is safe to broaden an empty search into
/// a fuzzy one: doing so would be surprising if the user had already narrowed
/// their search with additional constraints.
fn bare_exact_name(spec: &MatchSpec) -> Option<&PackageName> {
    let name = spec.name.as_exact()?;

    let is_bare = spec.version.is_none()
        && spec.build.is_none()
        && spec.build_number.is_none()
        && spec.file_name.is_none()
        && spec.channel.is_none()
        && spec.subdir.is_none()
        && spec.md5.is_none()
        && spec.sha256.is_none()
        && spec.url.is_none()
        && spec.license.is_none()
        && spec.license_family.is_none()
        && spec.extras.is_none()
        && spec.flags.is_none()
        && spec.condition.is_none()
        && spec.track_features.is_none()
        && spec.namespace.is_none();

    is_bare.then_some(name)
}
