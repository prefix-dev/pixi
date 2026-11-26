//! `pixi audit` against a mock conda-forge mirror and mock audit services.
//!
//! The audit only checks packages whose identity is conda-forge, so the
//! mock channel is served over local HTTP behind a mirror of conda-forge's
//! URL. The lock file records the conda-forge URL, as it would for the real
//! channel.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use axum::{Router, extract::Path as AxumPath, http::StatusCode, routing::get};
use clap::Parser;
use pixi_audit::types::{BatchResponse, BatchResult, OsvVulnerability, VulnRef};
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

async fn spawn(app: Router) -> Url {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port");
    let address = listener.local_addr().expect("local address");
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    Url::parse(&format!("http://{address}/")).expect("valid URL")
}

/// Serves the files below `root`.
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

/// An advisory affecting `package` below `fixed`, or every version without
/// one.
fn advisory(id: &str, package: &str, fixed: Option<&str>) -> OsvVulnerability {
    let mut events = vec![serde_json::json!({"introduced": "0"})];
    if let Some(fixed) = fixed {
        events.push(serde_json::json!({"fixed": fixed}));
    }
    serde_json::from_value(serde_json::json!({
        "id": id,
        "summary": format!("{id} in {package}"),
        "severity": [{"type": "CVSS_V3", "score": "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"}],
        "affected": [{
            "package": {"ecosystem": "conda-forge", "name": package},
            "ranges": [{"type": "ECOSYSTEM", "events": events}]
        }]
    }))
    .expect("valid advisory")
}

/// Mock vulnerability and threat-feed services.
struct AuditServices {
    url: Url,
    requests: Arc<AtomicUsize>,
}

/// Serves the vulnerability databases: `vulnerable` maps `name==version` to
/// the advisories reported for it.
async fn audit_services(
    vulnerable: &[(&str, &[&str])],
    advisories: Vec<OsvVulnerability>,
) -> AuditServices {
    let vulnerable: HashMap<String, Vec<String>> = vulnerable
        .iter()
        .map(|(package, ids)| {
            (
                package.to_string(),
                ids.iter().map(|id| id.to_string()).collect(),
            )
        })
        .collect();
    let requests = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&requests);
    let osv = pixi_audit_test_support::osv_router(
        move |queries| {
            counter.fetch_add(1, Ordering::SeqCst);
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
                            vulns: vulnerable
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
        },
        advisories,
    );
    let app = osv
        .merge(pixi_audit_test_support::epss_router(vec![]))
        .merge(pixi_audit_test_support::kev_router(
            serde_json::json!({"vulnerabilities": []}),
        ));
    AuditServices {
        url: spawn(app).await,
        requests,
    }
}

/// A workspace locking from the mock conda-forge mirror.
struct Workspace {
    pixi: PixiControl,
    platform: Subdir,
    _channel: pixi_test_utils::LocalChannel,
}

async fn workspace(manifest_body: &str, packages: &[(&str, &str)]) -> Workspace {
    let platform = Subdir::current().unwrap_or(Subdir::Linux64);
    let mut repo = MockRepoData::default();
    for (name, version) in packages {
        repo.add_package(
            Package::build(*name, version)
                .with_subdir(platform)
                .finish(),
        );
    }
    let channel = repo.into_channel().await.expect("mock channel");
    let mirror = serve_directory(channel.url().to_file_path().expect("a local channel")).await;
    let channel_url = unique_channel();
    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        channels = ["{channel_url}"]
        platforms = ["{platform}"]
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
        _channel: channel,
    }
}

/// Runs `pixi audit` with `args` against `services`.
async fn audit(
    pixi: &PixiControl,
    services: &Url,
    args: &[&str],
) -> miette::Result<std::process::ExitCode> {
    let manifest = pixi.manifest_path();
    let mut argv = vec![
        "audit".to_string(),
        "--manifest-path".to_string(),
        manifest.display().to_string(),
    ];
    argv.extend(args.iter().map(|arg| arg.to_string()));
    let mut args = pixi_cli::audit::Args::try_parse_from(argv).expect("valid arguments");
    args.config_source = isolated_config_source();
    args.config = pixi.config_cli();
    let cache = pixi.workspace_path().join("audit-cache");
    let url = services.as_str().to_string();
    temp_env::async_with_vars(
        [
            ("PIXI_AUDIT_BASILISK_URL", Some(url.clone())),
            ("PIXI_AUDIT_OSV_URL", Some(url.clone())),
            ("PIXI_AUDIT_EPSS_BASE_URL", Some(url.clone())),
            ("PIXI_AUDIT_KEV_URL", Some(format!("{url}kev.json"))),
            ("PIXI_AUDIT_TOKEN", None),
            ("PIXI_CACHE_AUDIT_DIR", Some(cache.display().to_string())),
        ],
        pixi_cli::audit::execute(args),
    )
    .await
}

/// The bytes of the manifest and lock file.
fn snapshot(pixi: &PixiControl) -> (Vec<u8>, Vec<u8>) {
    let root = pixi.workspace_path();
    (
        fs_err::read(root.join("pixi.toml")).expect("manifest"),
        fs_err::read(root.join("pixi.lock")).expect("lock file"),
    )
}

fn environments_dir(pixi: &PixiControl) -> PathBuf {
    pixi.workspace_path().join(".pixi").join("envs")
}

fn error_chain(err: &miette::Report) -> String {
    err.chain()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(": ")
}

async fn lock_with(workspace: &Workspace, constraints: &[&str]) {
    let mut lock = workspace.pixi.lock();
    for constraint in constraints {
        lock = lock.with_constraint(*constraint);
    }
    lock.await.expect("lock the workspace");
}

#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn audit_reports_without_writing_solving_or_installing() {
    let workspace = workspace(
        "[dependencies]\nopenssl = \"*\"\n",
        &[("openssl", "3.1.0"), ("openssl", "3.1.1")],
    )
    .await;
    lock_with(&workspace, &["openssl ==3.1.0"]).await;
    let before = snapshot(&workspace.pixi);
    let services = audit_services(
        &[("openssl==3.1.0", &["BSLK-1"])],
        vec![advisory("BSLK-1", "openssl", Some("3.1.1"))],
    )
    .await;

    for format in ["table", "by-package", "json", "sarif"] {
        let err = audit(&workspace.pixi, &services.url, &["--format", format])
            .await
            .expect_err("the vulnerable package fails the audit");
        assert_eq!(
            err.to_string(),
            "1 vulnerability match fails the audit policy"
        );
    }
    assert_eq!(snapshot(&workspace.pixi), before);
    assert!(!environments_dir(&workspace.pixi).exists());

    // A severity threshold above the finding passes, a manifest exception
    // passes, and filters select what is audited.
    audit(&workspace.pixi, &services.url, &["--ignore", "bslk-1"])
        .await
        .expect("ignored on the command line");
    let err = audit(
        &workspace.pixi,
        &services.url,
        &["--environment", "missing"],
    )
    .await
    .expect_err("unknown environments are rejected");
    assert_eq!(
        error_chain(&err),
        "the lock file contains no environment named 'missing'"
    );
}

#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn manifest_exceptions_and_expiry() {
    let workspace = workspace(
        r#"
        [dependencies]
        openssl = "*"

        [workspace.audit]
        ignore = [{ id = "CVE-2026-1", package = "openssl", expired-at = "2000-01-01" }]
        ignore-until-fixed = ["BSLK-NOFIX"]
        "#,
        &[("openssl", "3.1.0")],
    )
    .await;
    lock_with(&workspace, &[]).await;
    let services = audit_services(
        &[("openssl==3.1.0", &["BSLK-NOFIX"])],
        vec![advisory("BSLK-NOFIX", "openssl", None)],
    )
    .await;
    audit(&workspace.pixi, &services.url, &[])
        .await
        .expect("the unfixed advisory is ignored until fixed");

    let fixed = audit_services(
        &[("openssl==3.1.0", &["BSLK-NOFIX"])],
        vec![advisory("BSLK-NOFIX", "openssl", Some("3.1.1"))],
    )
    .await;
    audit(&workspace.pixi, &fixed.url, &[])
        .await
        .expect_err("a fix in conda-forge ends the exception");
}

#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn fix_replaces_the_lock_file_only_when_everything_passes() {
    let workspace = workspace(
        r#"
        [dependencies]
        openssl = "*"

        [feature.other.dependencies]
        zlib = "*"

        [environments]
        other = { features = ["other"], no-default-feature = true }
        "#,
        &[("openssl", "3.1.0"), ("openssl", "3.1.1"), ("zlib", "1.3")],
    )
    .await;
    lock_with(&workspace, &["openssl ==3.1.0"]).await;
    let (manifest, original_lock) = snapshot(&workspace.pixi);
    let platform = workspace.platform;

    // An unrelated finding without a fix blocks the fixable one: nothing is
    // solved or written.
    let blocked = audit_services(
        &[
            ("openssl==3.1.0", &["BSLK-1"]),
            ("zlib==1.3", &["BSLK-ZLIB"]),
        ],
        vec![
            advisory("BSLK-1", "openssl", Some("3.1.1")),
            advisory("BSLK-ZLIB", "zlib", None),
        ],
    )
    .await;
    let err = audit(&workspace.pixi, &blocked.url, &["--fix"])
        .await
        .expect_err("the unfixable finding blocks remediation");
    assert!(
        error_chain(&err).contains("zlib 1.3 has no fixed release for BSLK-ZLIB"),
        "{err:?}"
    );
    assert_eq!(
        snapshot(&workspace.pixi),
        (manifest.clone(), original_lock.clone())
    );

    // The fixed release is affected by another advisory: the candidate
    // audit rejects it.
    let regressed = audit_services(
        &[
            ("openssl==3.1.0", &["BSLK-1"]),
            ("openssl==3.1.1", &["BSLK-2"]),
        ],
        vec![
            advisory("BSLK-1", "openssl", Some("3.1.1")),
            advisory("BSLK-2", "openssl", Some("3.1.2")),
        ],
    )
    .await;
    let err = audit(&workspace.pixi, &regressed.url, &["--fix"])
        .await
        .expect_err("the candidate has a new finding");
    assert!(
        error_chain(&err).contains("BSLK-2 in openssl 3.1.1"),
        "{err:?}"
    );
    assert_eq!(
        snapshot(&workspace.pixi),
        (manifest.clone(), original_lock.clone())
    );
    assert!(!environments_dir(&workspace.pixi).exists());

    // A fix that passes replaces the lock file, and nothing else.
    let fixable = audit_services(
        &[("openssl==3.1.0", &["BSLK-1"])],
        vec![advisory("BSLK-1", "openssl", Some("3.1.1"))],
    )
    .await;
    audit(&workspace.pixi, &fixable.url, &["--fix"])
        .await
        .expect("the fix passes");
    let (manifest_after, lock_after) = snapshot(&workspace.pixi);
    assert_eq!(manifest_after, manifest);
    assert_ne!(lock_after, original_lock);
    assert!(!environments_dir(&workspace.pixi).exists());
    let lock = workspace.pixi.lock_file().await.expect("lock file");
    assert!(lock.contains_match_spec("default", platform, "openssl ==3.1.1"));
    assert!(lock.contains_match_spec("other", platform, "zlib ==1.3"));

    // A passing audit is a no-op.
    audit(&workspace.pixi, &fixable.url, &["--fix"])
        .await
        .expect("nothing to fix");
    assert_eq!(snapshot(&workspace.pixi), (manifest, lock_after));
}

#[tokio::test]
async fn offline_audits_send_no_requests() {
    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        channels = ["{CHANNEL}"]
        platforms = ["linux-64"]
        "#
    ))
    .expect("valid manifest");
    fs_err::write(
        pixi.workspace_path().join("pixi.lock"),
        format!(
            r#"version: 6
environments:
  default:
    channels:
    - url: {CHANNEL}/
    packages:
      linux-64:
      - conda: {CHANNEL}/linux-64/openssl-3.1.0-h0_0.conda
packages:
- conda: {CHANNEL}/linux-64/openssl-3.1.0-h0_0.conda
  sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
  md5: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
  size: 100
  timestamp: 1700000000000
"#
        ),
    )
    .expect("write the lock file");
    let services = audit_services(&[], vec![]).await;
    let err = audit(&pixi, &services.url, &[])
        .await
        .expect_err("vulnerability lookups need the network");
    assert!(error_chain(&err).contains("offline"), "{err:?}");
    assert_eq!(services.requests.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn pyproject_workspaces_audit_offline_without_requests_when_nothing_needs_a_lookup() {
    let pixi = PixiControl::from_pyproject_manifest(
        r#"
        [project]
        name = "audited"
        version = "0.1.0"

        [tool.pixi.workspace]
        channels = ["bioconda"]
        platforms = ["linux-64"]

        [tool.pixi.workspace.audit]
        fail-on = "critical"
        ignore = [{ package = "samtools", reason = "never audited anyway" }]
        "#,
    )
    .expect("valid manifest");
    fs_err::write(
        pixi.workspace_path().join("pixi.lock"),
        r#"version: 6
environments:
  default:
    channels:
    - url: https://conda.anaconda.org/bioconda/
    packages:
      linux-64:
      - conda: https://conda.anaconda.org/bioconda/linux-64/samtools-1.19-h0_0.conda
packages:
- conda: https://conda.anaconda.org/bioconda/linux-64/samtools-1.19-h0_0.conda
  sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
  md5: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
  size: 100
  timestamp: 1700000000000
"#,
    )
    .expect("write the lock file");
    let services = audit_services(&[], vec![]).await;
    audit(&pixi, &services.url, &["--json"])
        .await
        .expect("nothing needs a lookup, so the audit passes offline");
    assert_eq!(services.requests.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn missing_lock_files_and_filtered_fixes_are_rejected() {
    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        channels = ["{CHANNEL}"]
        platforms = ["linux-64"]
        "#
    ))
    .expect("valid manifest");
    let services = Url::parse("http://127.0.0.1:9/").expect("valid URL");
    let err = audit(&pixi, &services, &[])
        .await
        .expect_err("there is no lock file");
    assert!(
        err.to_string()
            .starts_with("the workspace has no lock file at"),
        "{err:?}"
    );
    assert!(!pixi.workspace_path().join("pixi.lock").exists());

    // Workspace options before the subcommand still reach it.
    assert!(
        pixi_cli::audit::Args::try_parse_from([
            "audit",
            "--manifest-path",
            "pixi.toml",
            "ignore",
            "CVE-1"
        ])
        .is_ok()
    );

    for filter in [["--environment", "default"], ["--platform", "linux-64"]] {
        let err =
            pixi_cli::audit::Args::try_parse_from(["audit", "--fix"].into_iter().chain(filter))
                .expect_err("filters do not apply to `--fix`");
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }
}

#[test]
fn removed_flags_are_rejected() {
    for flag in ["--ignore-unfixed", "--lock-file"] {
        assert!(pixi_cli::audit::Args::try_parse_from(["audit", flag]).is_err());
    }
    assert!(pixi_cli::audit::Args::try_parse_from(["audit", "pixi.lock"]).is_err());
}
