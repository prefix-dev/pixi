use indexmap::IndexSet;
use miette::IntoDiagnostic;
use pixi_config::Config;
use pixi_core::Workspace;
use pixi_utils::reqwest::build_lazy_reqwest_clients;
use rattler_conda_types::{
    Channel, MatchSpec, ParseStrictness, ParseStrictnessWithNameMatcher, Platform, RepoDataRecord,
};

pub async fn search(
    workspace: Option<&Workspace>,
    pattern: &str,
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

    // Parse with glob support
    let matchspec = MatchSpec::from_str(
        pattern,
        ParseStrictnessWithNameMatcher {
            parse_strictness: ParseStrictness::Lenient,
            exact_names_only: false, // Enables glob patterns like python*
        },
    )
    .into_diagnostic()?;

    // Query gateway - it handles glob matching internally
    let repo_data = gateway
        .query(
            channels.clone(),
            platforms,
            vec![matchspec.clone()],
        )
        .recursive(false)
        .await
        .into_diagnostic()?;

    // Collect and sort records
    let mut packages: Vec<RepoDataRecord> = Vec::new();
    for repo in repo_data {
        packages.extend(repo.iter().cloned());
    }

    if packages.is_empty() {
        return Err(miette::miette!(
            help = "Try glob patterns like 'python*' or '*numpy*'",
            "No packages found matching '{}'",
            pattern
        ));
    }

    // Sort by name, then by version within each package
    packages.sort();

    Ok(packages)
}
