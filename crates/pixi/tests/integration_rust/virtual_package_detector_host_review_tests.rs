use std::{path::Path, sync::Arc};

use pixi_config::{Config, GlobalConfigSource};
use pixi_core::{
    host::{DenyAll, HostDetection, HostDetector, WantedNames, manifest_channels},
    workspace::{DiscoveryStart, WorkspaceLocator},
};
use pixi_manifest::WorkspaceManifest;
use rattler_conda_types::{ChannelConfig, ChannelUrl, MatchSpec, ParseStrictness, Subdir};
use url::Url;

use crate::{
    common::PixiControl,
    virtual_package_detector_tests::{Detector, REPORT, counting_detector, write_channel_with},
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

async fn project_allow_cannot_authorize(global_decision: Option<&str>) {
    let channel_dir = tempfile::tempdir().unwrap();
    let counter = channel_dir.path().join("runs");
    write_channel_with(channel_dir.path(), &[counting_detector(&counter, REPORT)]).await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let subdir = Subdir::current().unwrap();
    let pixi = PixiControl::from_manifest(&format!(
        r#"[workspace]
name = "untrusted-consent"
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
    assert!(
        !counter.exists(),
        "project configuration executed its own detector"
    );
    assert_eq!(version(workspace.host(), "__test_good"), None);
}

#[tokio::test]
async fn project_allow_cannot_grant_consent() {
    project_allow_cannot_authorize(None).await;
}

#[tokio::test]
async fn project_allow_cannot_override_trusted_deny() {
    project_allow_cannot_authorize(Some("deny")).await;
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
        Detector { name: "bad-cuda", names: &["__cuda_arch", "__bad_other"], script: "echo '{\"version\":1,\"virtual_packages\":{\"__cuda_arch\":{\"version\":\"8.6\"},\"__bad_other\":{\"version\":\"1\"}}}'".into() },
        Detector { name: "override-detect", names: &["__review_override"], script: "exit 1".into() },
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
            script: format!("echo '{report}'"),
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
fn manifest_detector_channels_use_priority_then_manifest_order() {
    let root = tempfile::tempdir().unwrap();
    let manifest = WorkspaceManifest::from_toml_str_with_base_dir(
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
        root.path(),
    )
    .unwrap();
    let channels = manifest_channels(
        &manifest,
        &ChannelConfig::default_with_root_dir(root.path().to_path_buf()),
    )
    .unwrap();
    let expected: Vec<ChannelUrl> = ["feature", "higher", "equal", "lower"]
        .into_iter()
        .map(|name| {
            Url::parse(&format!("https://example.com/{name}/"))
                .unwrap()
                .into()
        })
        .collect();
    assert_eq!(channels, expected);
}
