use indexmap::IndexSet;
use miette::IntoDiagnostic;
use pixi_config::Config;
use pixi_core::Workspace;
use pixi_utils::reqwest::build_lazy_reqwest_clients;
use rattler_conda_types::{
    Channel, MatchSpec, PackageName, ParseStrictness, ParseStrictnessWithNameMatcher, Platform,
    RepoDataRecord,
};

pub async fn search(
    workspace: Option<&Workspace>,
    matchspec: MatchSpec,
    channels: IndexSet<Channel>,
    platforms: Vec<Platform>,
) -> miette::Result<Vec<RepoDataRecord>> {
    let client = if let Some(workspace) = workspace {
        workspace.authenticated_client()?.clone()
    } else {
        build_lazy_reqwest_clients(None, None)?.1
    };

    let config = Config::load_global();
    let gateway = config.gateway().with_client(client).finish();

    // Run a single repodata query for a match spec and collect the records.
    let run_query = |spec: MatchSpec| {
        let gateway = &gateway;
        let channels = channels.clone();
        let platforms = platforms.clone();
        async move {
            let repo_data = gateway
                .query(channels, platforms, vec![spec])
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

    let mut packages = run_query(matchspec.clone()).await?;

    // If an exact package-name search comes up empty, fall back to fuzzy
    // matching so the user doesn't have to know the precise name. We broaden
    // to a "contains" match (`*name*`) which also catches packages where the
    // term appears in the middle (e.g. `ros-jazzy-turtlesim` for `turtle`),
    // then rank names that *start* with the term first. This only kicks in for
    // a bare package name; if the user already provided a glob or extra
    // constraints we respect their input.
    if packages.is_empty()
        && let Some(name) = bare_exact_name(&matchspec)
    {
        let name = name.as_normalized().to_string();
        let fuzzy_spec = MatchSpec::from_str(
            &format!("*{name}*"),
            ParseStrictnessWithNameMatcher {
                parse_strictness: ParseStrictness::Lenient,
                exact_names_only: false,
            },
        )
        .into_diagnostic()?;

        let mut found = run_query(fuzzy_spec).await?;
        // Surface packages whose name starts with the search term before those
        // that merely contain it, keeping the natural (name/version) ordering
        // within each group.
        found.sort_by(|a, b| {
            let a_prefix = a.package_record.name.as_normalized().starts_with(&name);
            let b_prefix = b.package_record.name.as_normalized().starts_with(&name);
            b_prefix.cmp(&a_prefix).then_with(|| a.cmp(b))
        });
        packages = found;

        if !packages.is_empty() {
            return Ok(packages);
        }
    }

    if packages.is_empty() {
        return Err(miette::miette!(
            help = "Try glob patterns like 'python*' or '*numpy*'",
            "No packages found matching '{}'",
            matchspec
        ));
    }

    packages.sort();

    Ok(packages)
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
