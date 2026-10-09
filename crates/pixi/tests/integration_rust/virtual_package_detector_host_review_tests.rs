use std::{path::Path, slice::from_ref, sync::Arc};

use pixi_config::{Config, GlobalConfigSource};
use pixi_core::{
    host::{DenyAll, HostDetection, HostDetector, SkipReason, WantedNames},
    workspace::{DiscoveryStart, Workspace, WorkspaceLocator},
};
use pixi_manifest::{EnvironmentName, FeaturesExt};
use rattler_conda_types::{ChannelUrl, MatchSpec, ParseStrictness, Subdir};
use url::Url;

use crate::{
    common::PixiControl,
    virtual_package_detector_tests::{
        Detector, DetectorScript, REPORT, counting_detector, runs, write_channel_with,
    },
};

fn config(root: &Path, consents: &[(&Url, &str)]) -> Config {
    let mut contents = format!(
        "[cache]\nroot = {}\n",
        toml_edit::Value::from(root.to_string_lossy().as_ref())
    );
    if !consents.is_empty() {
        contents.push_str("[virtual-package-detectors.consent]\n");
    }
    for (channel, decision) in consents {
        contents.push_str(&format!(
            "{} = \"{decision}\"\n",
            toml_edit::Value::from(channel.as_str())
        ));
    }
    Config::from_toml(&contents, None).unwrap().0
}

fn version(host: &HostDetection, name: &str) -> Option<String> {
    host.capabilities()
        .iter()
        .find(|package| package.name.as_normalized() == name)
        .map(|package| package.version.to_string())
}

async fn project_allow_authorizes(global_decision: Option<&str>) {
    let channel_dir = tempfile::tempdir().unwrap();
    let counter = channel_dir.path().join("runs");
    write_channel_with(channel_dir.path(), &[counting_detector(&counter, REPORT)]).await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let subdir = Subdir::current().unwrap();
    let pixi = PixiControl::from_manifest(&format!(
        r#"[workspace]
name = "repository-consent"
channels = ["{channel}"]
platforms = [{{ platform = "{subdir}", test_good = "1" }}]
"#
    ))
    .unwrap();
    let local = pixi.workspace_path().join(".pixi/config.toml");
    let mut contents = fs_err::read_to_string(&local).unwrap();
    contents.push_str(&format!(
        "\n[cache]\nroot = {}\n[virtual-package-detectors.consent]\n{} = \"allow\"\n",
        toml_edit::Value::from(
            pixi.workspace_path()
                .join("cache")
                .to_string_lossy()
                .as_ref()
        ),
        toml_edit::Value::from(channel.as_str())
    ));
    fs_err::write(local, contents).unwrap();
    let source = match global_decision {
        Some(decision) => {
            let global = channel_dir.path().join("trusted.toml");
            fs_err::write(
                &global,
                format!(
                    "[virtual-package-detectors.consent]\n{} = \"{decision}\"\n",
                    toml_edit::Value::from(channel.as_str())
                ),
            )
            .unwrap();
            GlobalConfigSource::File(global)
        }
        None => GlobalConfigSource::None,
    };
    let workspace = WorkspaceLocator::default()
        .with_search_start(DiscoveryStart::ExplicitManifest(pixi.manifest_path()))
        .with_global_config_source(source)
        .with_cli_config(pixi.config_cli())
        .locate()
        .await
        .unwrap();
    assert_eq!(runs(&counter), 1);
    assert_eq!(
        version(workspace.host(), "__test_good").as_deref(),
        Some("1.2.3")
    );
}

#[tokio::test]
async fn project_allow_grants_consent() {
    project_allow_authorizes(None).await;
}

#[tokio::test]
async fn project_allow_overrides_user_deny() {
    project_allow_authorizes(Some("deny")).await;
}

#[tokio::test]
async fn denied_higher_channel_reserves_names_in_all_mode() {
    let root = tempfile::tempdir().unwrap();
    let high = root.path().join("high");
    let low = root.path().join("low");
    let counter = root.path().join("runs");
    write_channel_with(&high, &[Detector::default_report()]).await;
    write_channel_with(&low, &[counting_detector(&counter, REPORT)]).await;
    let high_url = Url::from_directory_path(high).unwrap();
    let low_url = Url::from_directory_path(low).unwrap();
    let detector = HostDetector::new(
        config(
            &root.path().join("cache"),
            &[(&high_url, "deny"), (&low_url, "allow")],
        ),
        Arc::new(DenyAll),
        None,
    )
    .unwrap();
    let host = detector
        .detect(&[high_url.into(), low_url.into()], WantedNames::All)
        .await
        .unwrap();
    assert!(
        !counter.exists(),
        "denial must not fall through to a lower channel"
    );
    assert_eq!(version(&host, "__test_good"), None);
}

#[tokio::test]
async fn all_mode_honors_override_without_consent() {
    let root = tempfile::tempdir().unwrap();
    let channel = root.path().join("channel");
    let counter = root.path().join("runs");
    write_channel_with(&channel, &[counting_detector(&counter, REPORT)]).await;
    let url = Url::from_directory_path(channel).unwrap();
    let detector = HostDetector::new(
        config(&root.path().join("cache"), &[]),
        Arc::new(DenyAll),
        None,
    )
    .unwrap();
    temp_env::async_with_vars([("CONDA_OVERRIDE_TEST_GOOD", Some("9.9"))], async {
        let host = detector
            .detect(&[url.into()], WantedNames::All)
            .await
            .unwrap();
        assert_eq!(version(&host, "__test_good").as_deref(), Some("9.9"));
    })
    .await;
    assert!(!counter.exists());
}

#[tokio::test]
async fn explicit_virtual_matchspec_wants_its_detector() {
    let root = tempfile::tempdir().unwrap();
    let channel = root.path().join("channel");
    write_channel_with(&channel, &[Detector::default_report()]).await;
    let url = Url::from_directory_path(channel).unwrap();
    let detector = HostDetector::new(
        config(&root.path().join("cache"), &[(&url, "allow")]),
        Arc::new(DenyAll),
        None,
    )
    .unwrap();
    let spec = MatchSpec::from_str("__test_good >=1", ParseStrictness::Strict).unwrap();
    let host = detector
        .detect_for_specs(&[url.into()], Subdir::current().unwrap(), &[spec])
        .await
        .unwrap();
    assert_eq!(version(&host, "__test_good").as_deref(), Some("1.2.3"));
}

#[tokio::test]
async fn explicit_foreign_solve_skips_detector_and_preserves_override() {
    let root = tempfile::tempdir().unwrap();
    let channel = root.path().join("channel");
    let counter = root.path().join("runs");
    write_channel_with(&channel, &[counting_detector(&counter, REPORT)]).await;
    let url = Url::from_directory_path(channel).unwrap();
    let detector = HostDetector::new(
        config(&root.path().join("cache"), &[(&url, "allow")]),
        Arc::new(DenyAll),
        None,
    )
    .unwrap();
    let native = Subdir::current().unwrap();
    let target = if native == Subdir::Linux64 {
        Subdir::LinuxAarch64
    } else {
        Subdir::Linux64
    };
    let channels = [url.into()];
    let spec = MatchSpec::from_str("needs-good", ParseStrictness::Strict).unwrap();
    temp_env::async_with_vars(
        [
            ("PIXI_OVERRIDE_PLATFORM", None::<&str>),
            ("CONDA_OVERRIDE_TEST_GOOD", None),
        ],
        async {
            let host = detector
                .detect_for_specs(&channels, target, from_ref(&spec))
                .await
                .unwrap();
            assert_eq!(host.subdir(), target);
            assert_eq!(version(&host, "__test_good"), None);
            assert!(host.detector_failures().is_empty());
            assert_eq!(host.skipped_detectors().len(), 1);
            assert!(matches!(
                host.skipped_detectors()[0].reason,
                SkipReason::TargetIsNotHost { .. }
            ));
        },
    )
    .await;
    temp_env::async_with_vars([("CONDA_OVERRIDE_TEST_GOOD", Some("9.9=foreign"))], async {
        let host = detector
            .detect_for_specs(&channels, target, &[spec])
            .await
            .unwrap();
        assert_eq!(host.subdir(), target);
        let package = host
            .capabilities()
            .iter()
            .find(|package| package.name.as_normalized() == "__test_good")
            .unwrap();
        assert_eq!(package.version.to_string(), "9.9");
        assert_eq!(package.build_string, "foreign");
    })
    .await;
    assert!(
        !counter.exists(),
        "a foreign solve must not execute a detector"
    );
}

#[tokio::test]
async fn empty_foreign_solve_still_describes_requested_target() {
    let root = tempfile::tempdir().unwrap();
    let detector = HostDetector::new(
        config(&root.path().join("cache"), &[]),
        Arc::new(DenyAll),
        None,
    )
    .unwrap();
    let target = if Subdir::current().unwrap() == Subdir::Linux64 {
        Subdir::LinuxAarch64
    } else {
        Subdir::Linux64
    };
    let host = detector.detect_for_specs(&[], target, &[]).await.unwrap();
    assert_eq!(host.subdir(), target);
}

#[tokio::test]
async fn relation_origin_consent_is_used_in_all_mode() {
    let root = tempfile::tempdir().unwrap();
    let overlay = root.path().join("overlay");
    let base = root.path().join("base");
    write_channel_with(&overlay, &[]).await;
    write_channel_with(&base, &[Detector::default_report()]).await;
    for subdir in [Subdir::current().unwrap(), Subdir::NoArch] {
        let path = overlay.join(subdir.as_str()).join("repodata.json");
        let mut repodata: serde_json::Value =
            serde_json::from_str(&fs_err::read_to_string(&path).unwrap()).unwrap();
        repodata["info"]["channel_relations"] = serde_json::json!({"base": "../base"});
        fs_err::write(path, serde_json::to_vec(&repodata).unwrap()).unwrap();
    }
    let overlay_url = Url::from_directory_path(overlay).unwrap();
    let base_url = Url::from_directory_path(base).unwrap();
    let detector = HostDetector::new(
        config(&root.path().join("cache"), &[(&base_url, "allow")]),
        Arc::new(DenyAll),
        None,
    )
    .unwrap();
    let host = detector
        .detect(&[overlay_url.into()], WantedNames::All)
        .await
        .unwrap();
    assert_eq!(version(&host, "__test_good").as_deref(), Some("1.2.3"));
}

#[tokio::test]
async fn invalid_detector_keeps_other_reports_and_overrides() {
    let root = tempfile::tempdir().unwrap();
    let channel = root.path().join("channel");
    write_channel_with(&channel, &[
        Detector::default_report(),
        Detector {
            name: "bad-cuda",
            names: &["__cuda_arch", "__bad_other"],
            script: DetectorScript::report(
                r#"{"version":1,"virtual_packages":{"__cuda_arch":{"version":"8.6"},"__bad_other":{"version":"1"}}}"#,
            ),
        },
        Detector {
            name: "override-detect",
            names: &["__review_override"],
            script: DetectorScript::failure(None, "", 1),
        },
    ]).await;
    let url = Url::from_directory_path(channel).unwrap();
    let cfg = config(&root.path().join("cache"), &[(&url, "allow")]);
    let detector = HostDetector::new(cfg, Arc::new(DenyAll), None).unwrap();
    temp_env::async_with_vars(
        [
            ("CONDA_OVERRIDE_CUDA", Some("")),
            ("CONDA_OVERRIDE_CUDA_ARCH", None),
            ("CONDA_OVERRIDE_REVIEW_OVERRIDE", Some("7.0")),
        ],
        async {
            let host = detector
                .detect(&[url.into()], WantedNames::All)
                .await
                .unwrap();
            assert_eq!(version(&host, "__test_good").as_deref(), Some("1.2.3"));
            assert_eq!(version(&host, "__review_override").as_deref(), Some("7.0"));
            assert_eq!(version(&host, "__cuda_arch"), None);
            assert_eq!(version(&host, "__bad_other"), None);
            assert!(
                host.detector_failures()
                    .iter()
                    .any(|failure| failure.detector.as_normalized() == "bad-cuda")
            );
        },
    )
    .await;
}

#[tokio::test]
async fn valid_detector_report_outgrows_manifest_platform_name() {
    let root = tempfile::tempdir().unwrap();
    let channel = root.path().join("channel");
    let names: Vec<String> = (0..12)
        .map(|index| format!("__review_capability_{index:02}"))
        .collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let packages: serde_json::Map<String, serde_json::Value> = names
        .iter()
        .map(|name| (name.clone(), serde_json::json!({"version":"1.2.3"})))
        .collect();
    let report = serde_json::json!({"version":1,"virtual_packages":packages});
    write_channel_with(
        &channel,
        &[Detector {
            name: "many-detect",
            names: &refs,
            script: DetectorScript::report(&report.to_string()),
        }],
    )
    .await;
    let url = Url::from_directory_path(channel).unwrap();
    let detector = HostDetector::new(
        config(&root.path().join("cache"), &[(&url, "allow")]),
        Arc::new(DenyAll),
        None,
    )
    .unwrap();
    let host = detector
        .detect(&[url.into()], WantedNames::All)
        .await
        .unwrap();
    for name in names {
        assert_eq!(version(&host, &name).as_deref(), Some("1.2.3"), "{name}");
    }
    assert!(host.platform().is_ok());
}

#[test]
fn default_detector_channels_ignore_unused_feature_priority() {
    let workspace = Workspace::from_str(
        Path::new("pixi.toml"),
        r#"[workspace]
name = "priority"
channels = [
    "https://example.com/lower",
    { channel = "https://example.com/higher", priority = 10 },
    { channel = "https://example.com/equal", priority = 10 },
]
platforms = ["linux-64"]

[feature.extra]
channels = [
    { channel = "https://example.com/feature", priority = 20 },
    { channel = "https://example.com/lower", priority = -10 },
]
"#,
    )
    .unwrap();
    let channels = workspace
        .default_environment()
        .channel_urls(&workspace.channel_config())
        .unwrap();
    let expected: Vec<ChannelUrl> = ["higher", "equal", "lower"]
        .into_iter()
        .map(|name| {
            Url::parse(&format!("https://example.com/{name}/"))
                .unwrap()
                .into()
        })
        .collect();
    assert_eq!(channels, expected);
}

#[tokio::test]
async fn unused_high_priority_feature_cannot_shadow_active_detector() {
    let root = tempfile::tempdir().unwrap();
    let high = root.path().join("high");
    let low = root.path().join("low");
    let counter = root.path().join("runs");
    write_channel_with(&high, &[Detector::default_report()]).await;
    write_channel_with(&low, &[counting_detector(&counter, REPORT)]).await;
    let high_url = Url::from_directory_path(high).unwrap();
    let low_url = Url::from_directory_path(low).unwrap();
    let subdir = Subdir::current().unwrap();
    let pixi = PixiControl::from_manifest(&format!(
        r#"[workspace]
name = "scoped-detector"
channels = ["{low_url}"]
platforms = [{{ name = "custom", platform = "{subdir}", test_good = "1" }}]

[dependencies]
needs-good = "*"

[feature.unused]
channels = [{{ channel = "{high_url}", priority = 100 }}]
"#
    ))
    .unwrap();
    let workspace = WorkspaceLocator::default()
        .with_search_start(DiscoveryStart::ExplicitManifest(pixi.manifest_path()))
        .with_global_config_source(GlobalConfigSource::None)
        .with_cli_config(config(
            &root.path().join("cache"),
            &[(&high_url, "deny"), (&low_url, "allow")],
        ))
        .locate()
        .await
        .unwrap();
    let environment = workspace.default_environment();
    assert_eq!(
        version(environment.host(), "__test_good").as_deref(),
        Some("1.2.3")
    );
    assert_eq!(
        environment
            .best_declared_platform()
            .unwrap()
            .name()
            .as_str(),
        "custom"
    );
    assert_eq!(
        environment
            .virtual_packages(environment.best_declared_platform().unwrap())
            .iter()
            .find(|package| package.name.as_normalized() == "__test_good")
            .unwrap()
            .version
            .to_string(),
        "1"
    );
    assert!(std::ptr::eq(workspace.host(), environment.host()));
    assert_eq!(fs_err::read_to_string(counter).unwrap().lines().count(), 1);
}

#[tokio::test]
async fn environment_channel_order_controls_ownership_and_equal_scopes_share_hosts() {
    let root = tempfile::tempdir().unwrap();
    let high = root.path().join("high");
    let low = root.path().join("low");
    write_channel_with(&high, &[Detector::default_report()]).await;
    write_channel_with(&low, &[Detector::default_report()]).await;
    let high_url = Url::from_directory_path(high).unwrap();
    let low_url = Url::from_directory_path(low).unwrap();
    let subdir = Subdir::current().unwrap();
    let pixi = PixiControl::from_manifest(&format!(
        r#"[workspace]
name = "ordered-detectors"
channels = []
platforms = [{{ name = "custom", platform = "{subdir}", test_good = "1" }}]

[feature.allowed]
channels = ["{low_url}"]

[feature.denied]
channels = ["{high_url}"]

[feature.priority-denied]
channels = [{{ channel = "{high_url}", priority = 100 }}]

[environments]
default = {{ features = ["allowed", "denied"], no-default-feature = true }}
reversed = {{ features = ["denied", "allowed"], no-default-feature = true }}
twin = {{ features = ["allowed", "denied"], no-default-feature = true }}
group-denied = {{ features = ["priority-denied"], no-default-feature = true, solve-group = "shared" }}
group-allowed = {{ features = ["allowed"], no-default-feature = true, solve-group = "shared" }}
"#
    ))
    .unwrap();
    let workspace = WorkspaceLocator::default()
        .with_search_start(DiscoveryStart::ExplicitManifest(pixi.manifest_path()))
        .with_global_config_source(GlobalConfigSource::None)
        .with_cli_config(config(
            &root.path().join("cache"),
            &[(&high_url, "deny"), (&low_url, "allow")],
        ))
        .locate()
        .await
        .unwrap();
    let allowed = workspace.default_environment();
    let reversed = workspace
        .environment(&EnvironmentName::Named("reversed".into()))
        .unwrap();
    let twin = workspace
        .environment(&EnvironmentName::Named("twin".into()))
        .unwrap();
    let group_denied = workspace
        .environment(&EnvironmentName::Named("group-denied".into()))
        .unwrap();
    let group_allowed = workspace
        .environment(&EnvironmentName::Named("group-allowed".into()))
        .unwrap();
    assert!(allowed.best_declared_platform().is_some());
    assert!(reversed.best_declared_platform().is_none());
    assert!(group_denied.best_declared_platform().is_none());
    assert!(group_allowed.best_declared_platform().is_none());
    assert!(std::ptr::eq(allowed.host(), twin.host()));
    assert!(std::ptr::eq(group_denied.host(), group_allowed.host()));
    assert_eq!(
        version(allowed.host(), "__test_good").as_deref(),
        Some("1.2.3")
    );
    assert_eq!(version(reversed.host(), "__test_good"), None);
}

#[tokio::test]
async fn source_only_dependencies_still_detect_declared_custom_capabilities() {
    let root = tempfile::tempdir().unwrap();
    let channel = root.path().join("channel");
    write_channel_with(&channel, &[Detector::default_report()]).await;
    let url = Url::from_directory_path(channel).unwrap();
    let subdir = Subdir::current().unwrap();
    let pixi = PixiControl::from_manifest(&format!(
        r#"[workspace]
name = "source-detector"
channels = ["{url}"]
platforms = [{{ platform = "{subdir}", test_good = "1" }}]
preview = ["pixi-build"]

[dependencies]
source-package = {{ path = "." }}
"#
    ))
    .unwrap();
    let workspace = WorkspaceLocator::default()
        .with_search_start(DiscoveryStart::ExplicitManifest(pixi.manifest_path()))
        .with_global_config_source(GlobalConfigSource::None)
        .with_cli_config(config(&root.path().join("cache"), &[(&url, "allow")]))
        .locate()
        .await
        .unwrap();
    assert_eq!(
        version(workspace.host(), "__test_good").as_deref(),
        Some("1.2.3")
    );
    assert!(
        workspace
            .default_environment()
            .best_declared_platform()
            .is_some()
    );
}
