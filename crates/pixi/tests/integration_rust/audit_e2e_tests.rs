//! End-to-end scenarios for `pixi audit`, `pixi audit --fix`, and
//! `pixi audit ignore`, run against a mock conda-forge mirror and mock
//! vulnerability services served over local HTTP.
//!
//! Every failing run checks that the manifest and lock file keep their exact
//! bytes and that no environment was installed.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use axum::{
    Json, Router,
    extract::{Path as AxumPath, Request},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
    routing::{any, get, post},
};
use clap::Parser;
use pixi_audit::types::{BatchResponse, BatchResult, OsvQuery, OsvVulnerability, VulnRef};
use pixi_test_utils::{MockRepoData, Package};
use rattler_conda_types::Subdir;
use url::Url;

use crate::common::{LockFileExt, PixiControl, isolated_config_source};

const CHANNEL: &str = "https://prefix.dev/conda-forge";

/// A conda-forge label unique to one workspace. Labels keep conda-forge's
/// identity, and the unique URL keeps the mock repodata out of the cache
/// entries of the real channel and of other tests.
fn unique_channel() -> String {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time after the epoch")
        .as_nanos();
    format!(
        "{CHANNEL}/label/audit-test-{}-{nanos}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    )
}
const CRITICAL: &str = "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H";
/// A well-formed Basilisk token: `bsk_` and 40 alphanumerics.
const TOKEN: &str = "bsk_ABCDEFGH0123456789abcdefghijklmnopqrstuv";

// ---------------------------------------------------------------------------
// Servers
// ---------------------------------------------------------------------------

async fn spawn(app: Router) -> Url {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port");
    let address = listener.local_addr().expect("local address");
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    Url::parse(&format!("http://{address}/")).expect("valid URL")
}

async fn serve_directory(root: PathBuf) -> Url {
    let root = Arc::new(root);
    spawn(Router::new().route(
        "/{*path}",
        get(move |AxumPath(path): AxumPath<String>| {
            let root = Arc::clone(&root);
            async move {
                if path.split('/').any(|part| part == "..") {
                    return Err(StatusCode::NOT_FOUND);
                }
                fs_err::tokio::read(root.join(path))
                    .await
                    .map_err(|_| StatusCode::NOT_FOUND)
            }
        }),
    ))
    .await
}

/// One request a mock server received.
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    authorization: Option<String>,
}

type RequestLog = Arc<Mutex<Vec<Seen>>>;

fn seen(request: &Request) -> Seen {
    Seen {
        path: request.uri().path().to_string(),
        authorization: request
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .map(|value| value.to_str().unwrap_or("<binary>").to_string()),
    }
}

/// Records every routed request of `app` in `log`.
fn recorded(app: Router, log: RequestLog) -> Router {
    app.layer(axum::middleware::from_fn(
        move |request: Request, next: Next| {
            let log = Arc::clone(&log);
            async move {
                log.lock().unwrap().push(seen(&request));
                next.run(request).await
            }
        },
    ))
}

/// What the `querybatch` endpoint answers: the zero-based index of the
/// batch request and its queries.
type QueryHandler = Arc<dyn Fn(usize, Vec<OsvQuery>) -> Response + Send + Sync>;

/// The `querybatch` endpoint, answered by `query`.
fn batch_route(query: QueryHandler) -> Router {
    let calls = Arc::new(AtomicUsize::new(0));
    Router::new().route(
        "/v1/querybatch",
        post(move |Json(body): Json<serde_json::Value>| {
            let query = Arc::clone(&query);
            let calls = Arc::clone(&calls);
            async move {
                let queries: Vec<OsvQuery> =
                    serde_json::from_value(body["queries"].clone()).expect("valid queries");
                query(calls.fetch_add(1, Ordering::SeqCst), queries)
            }
        }),
    )
}

/// The detail documents in `documents`; anything else is a 404.
fn documents_route(documents: Vec<OsvVulnerability>) -> Router {
    let documents: Arc<HashMap<String, OsvVulnerability>> = Arc::new(
        documents
            .into_iter()
            .map(|document| (document.id.clone(), document))
            .collect(),
    );
    Router::new().route(
        "/v1/vulns/{id}",
        get(move |AxumPath(id): AxumPath<String>| {
            let documents = Arc::clone(&documents);
            async move {
                documents
                    .get(&id)
                    .cloned()
                    .map(Json)
                    .ok_or(StatusCode::NOT_FOUND)
            }
        }),
    )
}

/// An OSV-protocol database.
fn database(query: QueryHandler, documents: Vec<OsvVulnerability>) -> Router {
    batch_route(query).merge(documents_route(documents))
}

fn threat_feeds() -> Router {
    pixi_audit_test_support::epss_router(vec![serde_json::json!({
        "cve": "CVE-2026-0001", "epss": "0.5", "percentile": "0.9", "date": "2026-11-26"
    })])
    .merge(pixi_audit_test_support::kev_router(
        serde_json::json!({"vulnerabilities": []}),
    ))
}

type Vulnerable<'a> = &'a [(&'a str, &'a [&'a str])];

/// `name==version` to the advisory ids reported for it.
fn table(vulnerable: Vulnerable<'_>) -> HashMap<String, Vec<String>> {
    vulnerable
        .iter()
        .map(|(package, ids)| {
            (
                package.to_string(),
                ids.iter().map(|id| id.to_string()).collect(),
            )
        })
        .collect()
}

fn answer(table: &HashMap<String, Vec<String>>, queries: &[OsvQuery]) -> BatchResponse {
    BatchResponse {
        results: queries
            .iter()
            .map(|query| {
                let key = format!(
                    "{}=={}",
                    query.package.name.as_deref().unwrap_or_default(),
                    query.version.as_deref().unwrap_or_default()
                );
                BatchResult {
                    vulns: table
                        .get(&key)
                        .into_iter()
                        .flatten()
                        .map(|id| VulnRef {
                            id: id.clone(),
                            modified: None,
                        })
                        .collect(),
                    next_page_token: None,
                }
            })
            .collect(),
    }
}

/// Answers every batch from `vulnerable`.
fn lookup(vulnerable: Vulnerable<'_>) -> QueryHandler {
    let table = table(vulnerable);
    Arc::new(move |_, queries| Json(answer(&table, &queries)).into_response())
}

/// Answers the first batch (the audit of the current lock file) from
/// `first`, and every later batch (the candidate) from `later`.
fn lookup_phased(first: Vulnerable<'_>, later: Vulnerable<'_>) -> QueryHandler {
    let first = table(first);
    let later = table(later);
    Arc::new(move |call, queries| {
        let table = if call == 0 { &first } else { &later };
        Json(answer(table, &queries)).into_response()
    })
}

/// Database and threat feeds on one server.
async fn services(query: QueryHandler, documents: Vec<OsvVulnerability>) -> Url {
    spawn(database(query, documents).merge(threat_feeds())).await
}

// ---------------------------------------------------------------------------
// Advisories
// ---------------------------------------------------------------------------

fn advisory_document(
    id: &str,
    package: &str,
    affected: serde_json::Value,
    severity: &str,
    aliases: &[&str],
) -> OsvVulnerability {
    let mut entry = serde_json::json!({
        "package": {"ecosystem": "conda-forge", "name": package},
    });
    entry
        .as_object_mut()
        .unwrap()
        .extend(affected.as_object().unwrap().clone());
    serde_json::from_value(serde_json::json!({
        "id": id,
        "aliases": aliases,
        "summary": format!("{id} in {package}"),
        "severity": [{"type": "CVSS_V3", "score": severity}],
        "affected": [entry],
    }))
    .expect("valid advisory")
}

/// One ECOSYSTEM range with `events`.
fn ranged(id: &str, package: &str, events: serde_json::Value, severity: &str) -> OsvVulnerability {
    advisory_document(
        id,
        package,
        serde_json::json!({"ranges": [{"type": "ECOSYSTEM", "events": events}]}),
        severity,
        &[],
    )
}

/// Every version below `fixed` is affected, or every version without one.
fn fixed_in(id: &str, package: &str, fixed: Option<&str>) -> OsvVulnerability {
    let mut events = vec![serde_json::json!({"introduced": "0"})];
    if let Some(fixed) = fixed {
        events.push(serde_json::json!({"fixed": fixed}));
    }
    ranged(id, package, serde_json::Value::Array(events), CRITICAL)
}

// ---------------------------------------------------------------------------
// Workspaces
// ---------------------------------------------------------------------------

struct Workspace {
    pixi: PixiControl,
    platform: Subdir,
    _channels: Vec<pixi_test_utils::LocalChannel>,
}

fn package(platform: Subdir, spec: &str, dependencies: &[(&str, &str)]) -> Package {
    let (name, version) = spec.split_once('=').expect("name=version");
    let mut builder = Package::build(name, version).with_subdir(platform);
    for (dependent, dependency) in dependencies {
        if *dependent == spec {
            builder = builder.with_dependency(*dependency);
        }
    }
    builder.finish()
}

/// A workspace locking from the mock conda-forge mirror holding `packages`
/// (`name=version`, with `dependencies` as `(name=version, spec)`), plus any
/// `extra_channels` after it.
async fn workspace_with(
    manifest_body: &str,
    packages: &[&str],
    dependencies: &[(&str, &str)],
    extra_channels: &[String],
) -> Workspace {
    let platform = Subdir::current().unwrap_or(Subdir::Linux64);
    let mut platforms = vec![platform];
    let mut repo = MockRepoData::default();
    for spec in packages {
        // `subdir:name=version` places a package on another platform, which
        // the workspace then also locks for.
        let (subdir, spec) = match spec.split_once(':') {
            Some((subdir, spec)) => (subdir.parse::<Subdir>().expect("a subdir"), spec),
            None => (platform, *spec),
        };
        if !platforms.contains(&subdir) {
            platforms.push(subdir);
        }
        repo.add_package(package(subdir, spec, dependencies));
    }
    let platforms = platforms
        .iter()
        .map(|platform| format!("\"{platform}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let channel = repo.into_channel().await.expect("mock channel");
    let mirror = serve_directory(channel.url().to_file_path().expect("a local channel")).await;
    let channel_url = unique_channel();
    let channels = std::iter::once(format!("\"{channel_url}\""))
        .chain(
            extra_channels
                .iter()
                .map(|channel| format!("\"{channel}\"")),
        )
        .collect::<Vec<_>>()
        .join(", ");
    let pixi = PixiControl::from_manifest(&format!(
        r#"
[workspace]
channels = [{channels}]
platforms = [{platforms}]
{manifest_body}
"#
    ))
    .expect("valid manifest")
    .with_network_access();
    fs_err::write(
        pixi.workspace_path().join(".pixi").join("config.toml"),
        format!(
            "[mirrors]\n\"{channel_url}\" = [\"{mirror}\"]\n\n[repodata-config.\"https://prefix.dev\"]\ndisable-sharded = true\n"
        ),
    )
    .expect("write the workspace config");
    Workspace {
        pixi,
        platform,
        _channels: vec![channel],
    }
}

async fn workspace(manifest_body: &str, packages: &[&str]) -> Workspace {
    workspace_with(manifest_body, packages, &[], &[]).await
}

async fn lock_with(workspace: &Workspace, constraints: &[&str]) {
    let mut lock = workspace.pixi.lock();
    for constraint in constraints {
        lock = lock.with_constraint(*constraint);
    }
    lock.await.expect("lock the workspace");
}

/// Where the audit sends its requests.
struct Endpoints {
    basilisk: Url,
    /// OSV, EPSS, and KEV.
    others: Url,
}

impl Endpoints {
    fn single(url: &Url) -> Self {
        Self {
            basilisk: url.clone(),
            others: url.clone(),
        }
    }
}

async fn audit_as(
    pixi: &PixiControl,
    endpoints: &Endpoints,
    token: Option<&str>,
    args: &[&str],
) -> miette::Result<std::process::ExitCode> {
    let mut argv = vec![
        "audit".to_string(),
        "--manifest-path".to_string(),
        pixi.manifest_path().display().to_string(),
    ];
    argv.extend(args.iter().map(|arg| arg.to_string()));
    let mut args = pixi_cli::audit::Args::try_parse_from(argv).expect("valid arguments");
    args.config_source = isolated_config_source();
    args.config = pixi.config_cli();
    let cache = pixi.workspace_path().join("audit-cache");
    let others = endpoints.others.as_str().to_string();
    temp_env::async_with_vars(
        [
            (
                "PIXI_AUDIT_BASILISK_URL",
                Some(endpoints.basilisk.as_str().to_string()),
            ),
            ("PIXI_AUDIT_OSV_URL", Some(others.clone())),
            ("PIXI_AUDIT_EPSS_BASE_URL", Some(others.clone())),
            ("PIXI_AUDIT_KEV_URL", Some(format!("{others}kev.json"))),
            ("PIXI_AUDIT_TOKEN", token.map(str::to_string)),
            ("PIXI_CACHE_AUDIT_DIR", Some(cache.display().to_string())),
        ],
        pixi_cli::audit::execute(args),
    )
    .await
}

async fn audit(
    pixi: &PixiControl,
    services: &Url,
    args: &[&str],
) -> miette::Result<std::process::ExitCode> {
    audit_as(pixi, &Endpoints::single(services), None, args).await
}

async fn audit_ignore(pixi: &PixiControl, args: &[&str]) -> miette::Result<()> {
    let mut argv = vec!["audit".to_string(), "ignore".to_string()];
    argv.extend(args.iter().map(|arg| arg.to_string()));
    argv.push("--manifest-path".to_string());
    argv.push(pixi.manifest_path().display().to_string());
    let mut args = pixi_cli::audit::Args::try_parse_from(argv)
        .map_err(|err| miette::miette!("invalid arguments: {err}"))?;
    args.config_source = isolated_config_source();
    args.config = pixi.config_cli();
    pixi_cli::audit::execute(args).await.map(|_| ())
}

/// The bytes of the manifest and lock file.
fn snapshot(pixi: &PixiControl) -> (Vec<u8>, Vec<u8>) {
    (
        fs_err::read(pixi.manifest_path()).expect("manifest"),
        fs_err::read(pixi.workspace_path().join("pixi.lock")).expect("lock file"),
    )
}

fn assert_untouched(pixi: &PixiControl, before: &(Vec<u8>, Vec<u8>)) {
    let after = snapshot(pixi);
    assert!(
        after.0 == before.0,
        "the manifest changed:\n{}",
        String::from_utf8_lossy(&after.0)
    );
    assert!(after.1 == before.1, "the lock file changed");
    assert_no_environments(pixi);
}

fn assert_no_environments(pixi: &PixiControl) {
    assert!(
        !pixi.workspace_path().join(".pixi").join("envs").exists(),
        "an environment was installed"
    );
}

fn error_chain(err: &miette::Report) -> String {
    err.chain()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(": ")
}

/// Runs a failing `pixi audit` and returns its error chain.
async fn audit_fails(pixi: &PixiControl, services: &Url, args: &[&str]) -> String {
    let err = audit(pixi, services, args)
        .await
        .expect_err("the audit must fail");
    error_chain(&err)
}

// ---------------------------------------------------------------------------
// `--fix`
// ---------------------------------------------------------------------------

#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn fix_writes_nothing_when_the_constrained_solve_fails() {
    // A prefix requirement has no exact interval form, so planning leaves it
    // to the solver, which then cannot satisfy both it and the fix.
    let workspace = workspace(
        "[dependencies]\nopenssl = \"3.1.*\"\n",
        &["openssl=3.1.0", "openssl=3.2.0"],
    )
    .await;
    lock_with(&workspace, &[]).await;
    let before = snapshot(&workspace.pixi);
    let url = services(
        lookup(&[("openssl==3.1.0", &["BSLK-SSL"])]),
        vec![fixed_in("BSLK-SSL", "openssl", Some("3.2.0"))],
    )
    .await;

    let chain = audit_fails(&workspace.pixi, &url, &["--fix"]).await;
    assert!(
        chain.contains("solving with the temporary constraints failed"),
        "{chain}"
    );
    assert_untouched(&workspace.pixi, &before);
}

#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn fix_follows_release_branches_per_environment() {
    let workspace = workspace(
        r#"
[dependencies]
openssl = "*"

[feature.lts.dependencies]
openssl = ">=3.0,<3.1"

[environments]
lts = { features = ["lts"], no-default-feature = true }
"#,
        &[
            "openssl=3.0.4",
            "openssl=3.0.5",
            "openssl=3.0.6",
            "openssl=3.1.0",
            "openssl=3.1.1",
            "openssl=3.1.2",
        ],
    )
    .await;
    lock_with(&workspace, &["openssl ==3.1.0|==3.0.4"]).await;
    let platform = workspace.platform;
    let lock = workspace.pixi.lock_file().await.expect("lock file");
    assert!(lock.contains_match_spec("default", platform, "openssl ==3.1.0"));
    assert!(lock.contains_match_spec("lts", platform, "openssl ==3.0.4"));
    let (manifest, _) = snapshot(&workspace.pixi);

    // Both branches are affected; 3.0.5 and 3.1.2 are the fixes.
    let branches = ranged(
        "BSLK-BRANCH",
        "openssl",
        serde_json::json!([
            {"introduced": "0"}, {"fixed": "3.0.5"},
            {"introduced": "3.1.0"}, {"fixed": "3.1.2"}
        ]),
        CRITICAL,
    );
    let url = services(
        lookup(&[
            ("openssl==3.0.4", &["BSLK-BRANCH"]),
            ("openssl==3.1.0", &["BSLK-BRANCH"]),
            ("openssl==3.1.1", &["BSLK-BRANCH"]),
        ]),
        vec![branches],
    )
    .await;

    audit(&workspace.pixi, &url, &["--fix"])
        .await
        .expect("each branch has a fix");
    assert_eq!(snapshot(&workspace.pixi).0, manifest);
    assert_no_environments(&workspace.pixi);
    let lock = workspace.pixi.lock_file().await.expect("lock file");
    assert!(
        lock.contains_match_spec("default", platform, "openssl ==3.1.2"),
        "the default environment moves to the fixed release of its branch"
    );
    assert!(
        lock.contains_match_spec("lts", platform, "openssl ==3.0.6"),
        "the lts environment stays on its branch"
    );
}

#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn fix_updates_every_environment_of_a_solve_group_and_no_default_feature_environments() {
    let workspace = workspace(
        r#"
[dependencies]
openssl = "*"

[feature.extra.dependencies]
zlib = "*"

[feature.z.dependencies]
zlib = "*"

[environments]
a = { features = ["extra"], solve-group = "group" }
b = { features = ["z"], solve-group = "group" }
minimal = { features = ["z"], no-default-feature = true }
"#,
        &["openssl=3.1.0", "openssl=3.1.1", "zlib=1.3", "zlib=1.3.1"],
    )
    .await;
    lock_with(&workspace, &["openssl ==3.1.0", "zlib ==1.3"]).await;
    let (manifest, original_lock) = snapshot(&workspace.pixi);
    let url = services(
        lookup(&[
            ("openssl==3.1.0", &["BSLK-SSL"]),
            ("zlib==1.3", &["BSLK-ZLIB"]),
        ]),
        vec![
            fixed_in("BSLK-SSL", "openssl", Some("3.1.1")),
            fixed_in("BSLK-ZLIB", "zlib", Some("1.3.1")),
        ],
    )
    .await;

    audit(&workspace.pixi, &url, &["--fix"])
        .await
        .expect("both packages have a fix");
    let (manifest_after, lock_after) = snapshot(&workspace.pixi);
    assert_eq!(manifest_after, manifest);
    assert_ne!(lock_after, original_lock);
    assert_no_environments(&workspace.pixi);
    let platform = workspace.platform;
    let lock = workspace.pixi.lock_file().await.expect("lock file");
    for environment in ["default", "a", "b"] {
        assert!(
            lock.contains_match_spec(environment, platform, "openssl ==3.1.1"),
            "{environment}"
        );
    }
    for environment in ["a", "b", "minimal"] {
        assert!(
            lock.contains_match_spec(environment, platform, "zlib ==1.3.1"),
            "{environment}"
        );
    }
    assert!(!lock.contains_conda_package("minimal", platform, "openssl"));
    assert!(!lock.contains_conda_package("default", platform, "zlib"));
}

#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn fix_is_rejected_when_the_candidate_loses_audit_coverage() {
    // The fixed release depends on a package that only another channel
    // provides, so the candidate contains a package no database covers.
    let platform = Subdir::current().unwrap_or(Subdir::Linux64);
    let mut other = MockRepoData::default();
    other.add_package(package(platform, "libextra=1.0", &[]));
    let other = other.into_channel().await.expect("mock channel");
    let mut workspace = workspace_with(
        "[dependencies]\nopenssl = \"*\"\n",
        &["openssl=3.1.0", "openssl=3.1.1"],
        &[("openssl=3.1.1", "libextra")],
        &[other.url().to_string()],
    )
    .await;
    workspace._channels.push(other);
    lock_with(&workspace, &["openssl ==3.1.0"]).await;
    let before = snapshot(&workspace.pixi);
    let url = services(
        lookup(&[("openssl==3.1.0", &["BSLK-SSL"])]),
        vec![fixed_in("BSLK-SSL", "openssl", Some("3.1.1"))],
    )
    .await;

    let chain = audit_fails(&workspace.pixi, &url, &["--fix"]).await;
    assert!(
        chain.contains("the candidate lock file audits less than the current one"),
        "{chain}"
    );
    assert!(chain.contains("libextra"), "{chain}");
    assert_untouched(&workspace.pixi, &before);
}

#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn fix_rejects_the_candidate_when_its_audit_fails() {
    let workspace = workspace(
        "[dependencies]\nopenssl = \"*\"\n",
        &["openssl=3.1.0", "openssl=3.1.1"],
    )
    .await;
    lock_with(&workspace, &["openssl ==3.1.0"]).await;
    let before = snapshot(&workspace.pixi);

    // The database fails once the candidate is audited.
    let flaky: QueryHandler = {
        let first = table(&[("openssl==3.1.0", &["BSLK-SSL"])]);
        Arc::new(move |call, queries| {
            if call == 0 {
                Json(answer(&first, &queries)).into_response()
            } else {
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        })
    };
    let url = services(flaky, vec![fixed_in("BSLK-SSL", "openssl", Some("3.1.1"))]).await;
    let chain = audit_fails(&workspace.pixi, &url, &["--fix"]).await;
    assert!(
        chain.contains("auditing the candidate lock file failed"),
        "{chain}"
    );
    assert_untouched(&workspace.pixi, &before);

    // The candidate is reported with an advisory whose document is missing.
    let url = services(
        lookup_phased(
            &[("openssl==3.1.0", &["BSLK-SSL"])],
            &[("openssl==3.1.1", &["BSLK-GHOST"])],
        ),
        vec![fixed_in("BSLK-SSL", "openssl", Some("3.1.1"))],
    )
    .await;
    let chain = audit_fails(&workspace.pixi, &url, &["--fix"]).await;
    assert!(
        chain.contains("auditing the candidate lock file failed"),
        "{chain}"
    );
    assert_untouched(&workspace.pixi, &before);
}

#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn fix_detects_a_manifest_changed_while_it_runs() {
    let workspace = workspace(
        "[dependencies]\nopenssl = \"*\"\n",
        &["openssl=3.1.0", "openssl=3.1.1"],
    )
    .await;
    lock_with(&workspace, &["openssl ==3.1.0"]).await;
    let (_, lock) = snapshot(&workspace.pixi);

    // A concurrent edit lands while the candidate is audited.
    let manifest_path = workspace.pixi.manifest_path();
    let first = table(&[("openssl==3.1.0", &["BSLK-SSL"])]);
    let editing: QueryHandler = Arc::new(move |call, queries| {
        if call == 1 {
            let mut contents = fs_err::read_to_string(&manifest_path).unwrap();
            contents.push_str("\n# edited concurrently\n");
            fs_err::write(&manifest_path, contents).unwrap();
        }
        let empty = HashMap::new();
        Json(answer(if call == 0 { &first } else { &empty }, &queries)).into_response()
    });
    let url = services(
        editing,
        vec![fixed_in("BSLK-SSL", "openssl", Some("3.1.1"))],
    )
    .await;

    let chain = audit_fails(&workspace.pixi, &url, &["--fix"]).await;
    assert!(
        chain.contains("changed while `--fix` was running"),
        "{chain}"
    );
    assert_eq!(snapshot(&workspace.pixi).1, lock, "the lock file changed");
    assert_no_environments(&workspace.pixi);
}

// ---------------------------------------------------------------------------
// Vulnerability lookups
// ---------------------------------------------------------------------------

#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn failing_vulnerability_lookups_fail_the_audit_and_the_fix() {
    let workspace = workspace("[dependencies]\nopenssl = \"*\"\n", &["openssl=3.1.0"]).await;
    lock_with(&workspace, &[]).await;
    let before = snapshot(&workspace.pixi);

    let reported = table(&[("openssl==3.1.0", &["BSLK-SSL"])]);
    let cases: Vec<(&str, Router, &str)> = vec![
        (
            "the batch lookup returns HTTP 500",
            database(
                Arc::new(|_, _| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
                vec![],
            ),
            "500",
        ),
        (
            "the detail document is missing",
            database(lookup(&[("openssl==3.1.0", &["BSLK-GHOST"])]), vec![]),
            "404",
        ),
        (
            "the detail lookup returns HTTP 500",
            {
                let reported = reported.clone();
                Router::new()
                    .route(
                        "/v1/vulns/{id}",
                        get(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
                    )
                    .merge(batch_route(Arc::new(move |_, queries| {
                        Json(answer(&reported, &queries)).into_response()
                    })))
            },
            "500",
        ),
        (
            "the batch lookup returns fewer results than queries",
            database(
                Arc::new(|_, _| Json(BatchResponse { results: vec![] }).into_response()),
                vec![],
            ),
            "expected 1 results, got 0",
        ),
        (
            "the batch lookup returns invalid JSON",
            database(Arc::new(|_, _| "not json".into_response()), vec![]),
            "failed to parse the response",
        ),
        (
            "the batch lookup paginates forever",
            database(
                Arc::new(|_, queries: Vec<OsvQuery>| {
                    Json(BatchResponse {
                        results: queries
                            .iter()
                            .map(|_| BatchResult {
                                vulns: vec![],
                                next_page_token: Some("again".to_string()),
                            })
                            .collect(),
                    })
                    .into_response()
                }),
                vec![],
            ),
            "more than 32 result pages",
        ),
    ];

    for (case, router, expected) in cases {
        let url = spawn(router.merge(threat_feeds())).await;
        for args in [&[][..], &["--fix"][..], &["--json"][..]] {
            let chain = audit_fails(&workspace.pixi, &url, args).await;
            assert!(chain.contains(expected), "{case} ({args:?}): {chain}");
            assert_untouched(&workspace.pixi, &before);
        }
    }
}

// ---------------------------------------------------------------------------
// Credentials
// ---------------------------------------------------------------------------

#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn the_token_is_sent_only_to_the_basilisk_origin() {
    let workspace = workspace("[dependencies]\nopenssl = \"*\"\n", &["openssl=3.1.0"]).await;
    lock_with(&workspace, &[]).await;
    let mut document = fixed_in("BSLK-SSL", "openssl", None);
    document.aliases = vec!["CVE-2026-0001".to_string()];

    let basilisk_log = RequestLog::default();
    let others_log = RequestLog::default();
    let basilisk = spawn(recorded(
        database(lookup(&[("openssl==3.1.0", &["BSLK-SSL"])]), vec![document]),
        Arc::clone(&basilisk_log),
    ))
    .await;
    let others = spawn(recorded(
        database(lookup(&[]), vec![]).merge(threat_feeds()),
        Arc::clone(&others_log),
    ))
    .await;
    assert_ne!(basilisk.origin(), others.origin());

    let err = audit_as(
        &workspace.pixi,
        &Endpoints { basilisk, others },
        Some(TOKEN),
        &[],
    )
    .await
    .expect_err("the advisory fails the audit");
    assert_eq!(
        err.to_string(),
        "1 vulnerability match fails the audit policy"
    );
    assert!(!format!("{err:?}").contains(TOKEN));

    let basilisk_requests = basilisk_log.lock().unwrap().clone();
    let paths: Vec<_> = basilisk_requests
        .iter()
        .map(|seen| seen.path.as_str())
        .collect();
    assert_eq!(paths, ["/v1/querybatch", "/v1/vulns/BSLK-SSL"]);
    let expected = format!("Bearer {TOKEN}");
    for request in &basilisk_requests {
        assert_eq!(request.authorization.as_deref(), Some(expected.as_str()));
    }

    let others_requests = others_log.lock().unwrap().clone();
    assert!(
        others_requests.iter().any(|seen| seen.path == "/kev.json"),
        "{others_requests:?}"
    );
    assert!(
        others_requests
            .iter()
            .any(|seen| seen.path == "/data/v1/epss"),
        "{others_requests:?}"
    );
    for request in &others_requests {
        assert_eq!(request.authorization, None, "{request:?}");
    }
}

#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn basilisk_redirects_are_not_followed() {
    let workspace = workspace("[dependencies]\nopenssl = \"*\"\n", &["openssl=3.1.0"]).await;
    lock_with(&workspace, &[]).await;
    let before = snapshot(&workspace.pixi);

    // Everything the redirect target receives, routed or not.
    let target_log = RequestLog::default();
    let target = {
        let log = Arc::clone(&target_log);
        spawn(Router::new().fallback(move |request: Request| {
            let log = Arc::clone(&log);
            async move {
                log.lock().unwrap().push(seen(&request));
                StatusCode::NOT_FOUND
            }
        }))
        .await
    };
    let redirect_to = |target: &Url, path: &str| {
        Redirect::temporary(target.join(path.trim_start_matches('/')).unwrap().as_str())
            .into_response()
    };

    // The batch lookup redirects.
    let redirecting = {
        let target = target.clone();
        spawn(Router::new().route(
            "/{*path}",
            any(move |AxumPath(path): AxumPath<String>| {
                let target = target.clone();
                async move { redirect_to(&target, &path) }
            }),
        ))
        .await
    };
    let feeds = spawn(threat_feeds()).await;
    let err = audit_as(
        &workspace.pixi,
        &Endpoints {
            basilisk: redirecting,
            others: feeds.clone(),
        },
        Some(TOKEN),
        &[],
    )
    .await
    .expect_err("a redirect is not followed");
    let chain = error_chain(&err);
    assert!(chain.contains("307"), "{chain}");
    assert!(!chain.contains(TOKEN), "{chain}");
    assert_untouched(&workspace.pixi, &before);

    // Only the detail lookup redirects.
    let detail_redirect = {
        let target = target.clone();
        spawn(
            Router::new()
                .route(
                    "/v1/vulns/{id}",
                    get(move |AxumPath(id): AxumPath<String>| {
                        let target = target.clone();
                        async move { redirect_to(&target, &format!("v1/vulns/{id}")) }
                    }),
                )
                .merge(batch_route(lookup(&[("openssl==3.1.0", &["BSLK-SSL"])]))),
        )
        .await
    };
    for args in [&[][..], &["--fix"][..]] {
        let err = audit_as(
            &workspace.pixi,
            &Endpoints {
                basilisk: detail_redirect.clone(),
                others: feeds.clone(),
            },
            Some(TOKEN),
            args,
        )
        .await
        .expect_err("a redirected detail lookup is not followed");
        assert!(error_chain(&err).contains("307"), "{err:?}");
        assert_untouched(&workspace.pixi, &before);
    }

    let reached = target_log.lock().unwrap().clone();
    assert!(reached.is_empty(), "the redirect was followed: {reached:?}");
}

// ---------------------------------------------------------------------------
// `[workspace.audit]` and `pixi audit ignore`
// ---------------------------------------------------------------------------

#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn rules_recorded_by_ignore_are_honoured_by_the_audit() {
    let workspace = workspace(
        "[dependencies]\nopenssl = \"*\"\nzlib = \"*\"\n",
        &["openssl=3.1.0", "zlib=1.3"],
    )
    .await;
    lock_with(&workspace, &[]).await;
    let url = services(
        lookup(&[
            ("openssl==3.1.0", &["BSLK-SSL"]),
            ("zlib==1.3", &["BSLK-ZLIB"]),
        ]),
        vec![
            fixed_in("BSLK-SSL", "openssl", Some("3.1.1")),
            fixed_in("BSLK-ZLIB", "zlib", None),
        ],
    )
    .await;
    let err = audit(&workspace.pixi, &url, &[])
        .await
        .expect_err("both advisories fail");
    assert_eq!(
        err.to_string(),
        "2 vulnerability matches fail the audit policy"
    );

    // An until-fixed rule for an advisory with a fix does not apply.
    audit_ignore(&workspace.pixi, &["bslk-ssl", "--until-fixed"])
        .await
        .unwrap();
    audit_ignore(&workspace.pixi, &["BSLK-ZLIB", "--until-fixed"])
        .await
        .unwrap();
    let err = audit(&workspace.pixi, &url, &[])
        .await
        .expect_err("the fixed advisory still fails");
    assert_eq!(
        err.to_string(),
        "1 vulnerability match fails the audit policy"
    );

    // A version-scoped rule that does not cover the locked version.
    audit_ignore(
        &workspace.pixi,
        &["BSLK-SSL", "--package", "openssl", "--version", "3.0.0"],
    )
    .await
    .unwrap();
    audit(&workspace.pixi, &url, &[])
        .await
        .expect_err("the rule does not cover 3.1.0");

    audit_ignore(
        &workspace.pixi,
        &[
            "BSLK-SSL",
            "--package",
            "openssl",
            "--up-to-version",
            "3.1.0",
        ],
    )
    .await
    .unwrap();
    let before = snapshot(&workspace.pixi);
    audit(&workspace.pixi, &url, &["--fix"])
        .await
        .expect("every match is ignored, so `--fix` has nothing to do");
    assert_untouched(&workspace.pixi, &before);
}

#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn fix_upgrades_the_dependent_that_holds_a_vulnerable_transitive_dependency() {
    let workspace = workspace_with(
        "[dependencies]\napp = \"*\"\n",
        &["app=1.0", "app=1.1", "openssl=3.1.0", "openssl=3.1.1"],
        &[
            ("app=1.0", "openssl ==3.1.0"),
            ("app=1.1", "openssl >=3.1.1"),
        ],
        &[],
    )
    .await;
    lock_with(&workspace, &["app ==1.0"]).await;
    let (manifest, _) = snapshot(&workspace.pixi);
    let url = services(
        lookup(&[("openssl==3.1.0", &["BSLK-SSL"])]),
        vec![fixed_in("BSLK-SSL", "openssl", Some("3.1.1"))],
    )
    .await;
    audit(&workspace.pixi, &url, &["--fix"])
        .await
        .expect("the solver may move the dependent");
    assert_eq!(snapshot(&workspace.pixi).0, manifest);
    assert_no_environments(&workspace.pixi);
    let lock = workspace.pixi.lock_file().await.expect("lock file");
    assert!(lock.contains_match_spec("default", workspace.platform, "app ==1.1"));
    assert!(lock.contains_match_spec("default", workspace.platform, "openssl ==3.1.1"));
}
