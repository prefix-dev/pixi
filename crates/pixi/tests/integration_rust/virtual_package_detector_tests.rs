//! Host detection with channel-registered virtual package detectors.
//!
//! Every test builds a local channel with one or more `noarch` detector
//! packages whose executables run a shell script, registers them in the
//! channel's repodata, and locates a workspace against that channel.

use std::{path::Path, sync::Arc, time::Instant};

use pixi_config::{Config, GlobalConfigSource};
use pixi_core::{
    host::{
        DenyAll, DetectedValue, DetectionSource, HostDetection, HostDetector, SkipReason,
        WantedNames,
    },
    workspace::{DiscoveryStart, Workspace, WorkspaceLocator},
};
use pixi_test_utils::{MockRepoData, Package};
use rattler_conda_types::{
    ChannelUrl, MatchSpec, ParseStrictness, Subdir,
    virtual_package_detector::DetectorRegistrationMetadata,
};
use url::Url;

use crate::common::PixiControl;

pub(crate) const DETECTOR: &str = "good-detect";
pub(crate) const REPORT: &str =
    r#"{"version": 1, "virtual_packages": {"__test_good": {"version": "1.2.3"}}}"#;

/// A detector package and its platform-specific executable scripts.
pub(crate) struct Detector<'a> {
    pub(crate) name: &'a str,
    pub(crate) names: &'a [&'a str],
    pub(crate) script: DetectorScript,
}

impl<'a> Detector<'a> {
    pub(crate) fn good(script: DetectorScript) -> Self {
        Self {
            name: DETECTOR,
            names: &["__test_good"],
            script,
        }
    }

    /// The default detector: prints [`REPORT`].
    pub(crate) fn default_report() -> Self {
        Self::good(DetectorScript::report(REPORT))
    }
}

pub(crate) struct DetectorScript {
    unix: String,
    windows: Option<String>,
}

impl DetectorScript {
    pub(crate) fn report(report: &str) -> Self {
        Self {
            unix: format!("printf '%s\\n' {}", unix_quote(report)),
            windows: Some(format!("{}\r\nexit /b 0", windows_echo(report))),
        }
    }

    fn counting(counter: &Path, report: &str) -> Self {
        let mut script = Self::report(report);
        script.unix = format!(
            "printf 'run\\n' >> {}\n{}",
            unix_quote(&counter.to_string_lossy()),
            script.unix
        );
        script.windows = script
            .windows
            .map(|body| format!(">>{} echo run\r\n{body}", windows_quote_path(counter)));
        script
    }

    pub(crate) fn failure(counter: Option<&Path>, diagnostic: &str, exit_code: u8) -> Self {
        let mut unix = format!(
            "printf '%s\\n' {} >&2\nexit {exit_code}",
            unix_quote(diagnostic)
        );
        let mut windows = format!("{} >&2\r\nexit /b {exit_code}", windows_echo(diagnostic));
        if let Some(counter) = counter {
            unix = format!(
                "printf 'run\\n' >> {}\n{unix}",
                unix_quote(&counter.to_string_lossy())
            );
            windows = format!(">>{} echo run\r\n{windows}", windows_quote_path(counter));
        }
        Self {
            unix,
            windows: Some(windows),
        }
    }

    fn delayed_report(report: &str) -> Self {
        let mut script = Self::report(report);
        script.unix = format!("sleep 20\n{}", script.unix);
        script.windows = script
            .windows
            .map(|body| format!("ping -n 21 127.0.0.1 >nul\r\n{body}"));
        script
    }

    #[cfg(unix)]
    pub(crate) fn unix_only(script: impl Into<String>) -> Self {
        Self {
            unix: script.into(),
            windows: None,
        }
    }
}

fn unix_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn windows_quote_path(path: &Path) -> String {
    format!("\"{}\"", path.to_string_lossy().replace('%', "%%"))
}

fn windows_echo(value: &str) -> String {
    let mut command = String::from("echo(");
    for character in value.chars() {
        match character {
            '%' => command.push_str("%%"),
            '^' | '"' | '&' | '|' | '<' | '>' | '(' | ')' => {
                command.push('^');
                command.push(character);
            }
            _ => command.push(character),
        }
    }
    command
}

/// Writes a channel registering `detectors`, plus a `needs-good` package
/// that depends on `__test_good >=1` and a `plain` package without any
/// virtual package reference.
pub(crate) async fn write_channel_with(channel: &Path, detectors: &[Detector<'_>]) {
    let mut repodata = MockRepoData::default();
    let mut registrations = DetectorRegistrationMetadata::new();
    for detector in detectors {
        let mut package = Package::build(detector.name, "1.0")
            .with_subdir(Subdir::NoArch)
            .with_materialize(true)
            .with_file(
                Path::new("bin").join(detector.name),
                format!("#!/bin/sh\n{}\n", detector.script.unix),
                true,
            );
        if let Some(script) = &detector.script.windows {
            package = package.with_file(
                Path::new("Scripts").join(format!("{}.bat", detector.name)),
                format!("@echo off\r\nsetlocal DisableDelayedExpansion\r\n{script}\r\n"),
                true,
            );
        }
        let package = package.finish();
        repodata.add_package(package);
        registrations.insert(
            detector.name.to_string(),
            detector.names.iter().map(|name| name.to_string()).collect(),
        );
    }
    repodata.add_package(
        Package::build("needs-good", "2.0")
            .with_subdir(Subdir::NoArch)
            .with_materialize(true)
            .with_dependency("__test_good >=1")
            .with_file(
                Path::new("bin").join("needs-good"),
                "#!/bin/sh\necho needs-good\n",
                true,
            )
            .with_file(
                Path::new("Scripts").join("needs-good.bat"),
                "@echo off\r\necho needs-good\r\nexit /b 0\r\n",
                true,
            )
            .finish(),
    );
    repodata.add_package(
        Package::build("plain", "2.0")
            .with_subdir(Subdir::NoArch)
            .with_materialize(true)
            .finish(),
    );
    repodata
        .with_virtual_package_detectors(registrations)
        .write_repodata(channel)
        .await
        .unwrap();
}

/// Writes a channel registering [`DETECTOR`] for `__test_good`.
async fn write_channel(channel: &Path) {
    write_channel_with(channel, &[Detector::default_report()]).await;
}

/// A workspace on `subdir` whose platform declares canonical detector names,
/// using `channel`, with the detector cache under the workspace.
fn workspace_declaring(channel: &Url, subdir: Subdir, declares: &[(&str, &str)]) -> PixiControl {
    let platform = if declares.is_empty() {
        format!(r#""{subdir}""#)
    } else {
        let entries = declares
            .iter()
            .map(|(name, version)| {
                format!(
                    r#"{} = "{version}""#,
                    name.strip_prefix("__")
                        .expect("canonical virtual-package name")
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(r#"{{ platform = "{subdir}", {entries} }}"#)
    };
    let pixi = PixiControl::from_manifest(&format!(
        r#"
[workspace]
name = "detectors"
channels = ["{channel}"]
platforms = [{platform}]
"#
    ))
    .unwrap();
    let cache = pixi.workspace_path().join("cache");
    append_config(
        &pixi,
        &format!(
            "[cache]\nroot = {}\nvirtual-package-detectors = {}\n",
            toml_edit::Value::from(cache.to_string_lossy().as_ref()),
            toml_edit::Value::from(detector_cache(&pixi).to_string_lossy().as_ref())
        ),
    );
    fs_err::write(pixi.workspace_path().join("trusted-config.toml"), "").unwrap();
    pixi
}

/// A workspace on the current subdir whose platform declares `test_good`,
/// using `channel`, with the detector cache under the workspace.
fn workspace_for(channel: &Url, declares_test_good: bool) -> PixiControl {
    let subdir = Subdir::current().unwrap();
    let declares: &[(&str, &str)] = if declares_test_good {
        &[("__test_good", "1.0")]
    } else {
        &[]
    };
    workspace_declaring(channel, subdir, declares)
}

fn append_config(pixi: &PixiControl, snippet: &str) {
    let config = pixi.workspace_path().join(".pixi").join("config.toml");
    let mut contents = fs_err::read_to_string(&config).unwrap();
    contents.push('\n');
    contents.push_str(snippet);
    fs_err::write(&config, contents).unwrap();
}

/// Appends a trusted channel-wide consent decision.
fn store_consent(pixi: &PixiControl, channel: &Url, decision: &str) {
    let snippet = format!(
        "[virtual-package-detectors.consent]\n{} = \"{decision}\"\n",
        toml_edit::Value::from(channel.as_str())
    );
    let path = pixi.workspace_path().join("trusted-config.toml");
    let mut contents = fs_err::read_to_string(&path).unwrap();
    contents.push('\n');
    contents.push_str(&snippet);
    fs_err::write(path, contents).unwrap();
}

async fn try_locate(pixi: &PixiControl) -> miette::Result<Workspace> {
    WorkspaceLocator::default()
        .with_search_start(DiscoveryStart::ExplicitManifest(pixi.manifest_path()))
        .with_global_config_source(GlobalConfigSource::File(
            pixi.workspace_path().join("trusted-config.toml"),
        ))
        .with_cli_config(pixi.config_cli())
        .locate()
        .await
        .map_err(miette::Report::new)
}

async fn locate(pixi: &PixiControl) -> Workspace {
    try_locate(pixi).await.unwrap()
}

/// The detector cache directory of a workspace built by [`workspace_for`].
fn detector_cache(pixi: &PixiControl) -> std::path::PathBuf {
    pixi.workspace_path()
        .join("cache")
        .join(pixi_consts::consts::VIRTUAL_PACKAGE_DETECTORS_CACHE_DIR)
}

/// The version of `__test_good` the host platform reports, if any.
fn test_good_version(host: &HostDetection) -> Option<String> {
    host.platform()
        .unwrap()
        .customised_virtual_packages()
        .into_iter()
        .find(|package| package.name.as_source() == "__test_good")
        .map(|package| package.version.to_string())
}

/// How often the counter script has run.
pub(crate) fn runs(counter: &Path) -> usize {
    fs_err::read_to_string(counter)
        .map(|contents| contents.lines().count())
        .unwrap_or(0)
}

/// A detector that appends a line to `counter` on every run, then prints
/// `report`.
pub(crate) fn counting_detector(counter: &Path, report: &str) -> Detector<'static> {
    Detector::good(DetectorScript::counting(counter, report))
}

#[tokio::test]
async fn an_allowed_detector_provides_the_declared_name() {
    let channel_dir = tempfile::tempdir().unwrap();
    write_channel(channel_dir.path()).await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let pixi = workspace_for(&channel, true);
    store_consent(&pixi, &channel, "allow");

    let workspace = locate(&pixi).await;
    let host = workspace.host();
    assert!(
        host.detector_failures().is_empty(),
        "{:?}",
        host.detector_failures()
    );
    assert!(
        host.skipped_detectors().is_empty(),
        "{:?}",
        host.skipped_detectors()
    );
    let results = host.detector_results();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name.as_source(), "__test_good");
    assert_eq!(test_good_version(host).as_deref(), Some("1.2.3"));
}

#[tokio::test]
async fn a_detector_registered_only_in_noarch_provides_the_declared_name() {
    let channel_dir = tempfile::tempdir().unwrap();
    write_channel(channel_dir.path()).await;
    let native_repodata = channel_dir
        .path()
        .join(Subdir::current().unwrap().as_str())
        .join("repodata.json");
    let mut repodata: serde_json::Value =
        serde_json::from_slice(&fs_err::read(&native_repodata).unwrap()).unwrap();
    repodata["info"]
        .as_object_mut()
        .unwrap()
        .remove("virtual_package_detectors");
    fs_err::write(&native_repodata, serde_json::to_vec(&repodata).unwrap()).unwrap();
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let pixi = workspace_for(&channel, true);
    store_consent(&pixi, &channel, "allow");

    let workspace = locate(&pixi).await;
    assert_eq!(
        test_good_version(workspace.host()).as_deref(),
        Some("1.2.3")
    );
}

#[tokio::test]
async fn a_detector_without_a_decision_is_skipped() {
    let channel_dir = tempfile::tempdir().unwrap();
    let counter = channel_dir.path().join("runs");
    write_channel_with(channel_dir.path(), &[counting_detector(&counter, REPORT)]).await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let pixi = workspace_for(&channel, true);

    let workspace = locate(&pixi).await;
    let host = workspace.host();
    assert!(host.detector_results().is_empty());
    let skipped = host.skipped_detectors();
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].detector.as_source(), DETECTOR);
    assert_eq!(skipped[0].reason, SkipReason::ConsentDenied);
    assert_eq!(test_good_version(host), None);
    assert_eq!(runs(&counter), 0, "an undecided detector must not execute");
}

#[tokio::test]
async fn a_denied_detector_is_skipped_without_installing_anything() {
    let channel_dir = tempfile::tempdir().unwrap();
    write_channel(channel_dir.path()).await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let pixi = workspace_for(&channel, true);
    store_consent(&pixi, &channel, "deny");

    let workspace = locate(&pixi).await;
    let host = workspace.host();
    assert!(host.detector_results().is_empty());
    assert!(host.detector_failures().is_empty());
    let skipped = host.skipped_detectors();
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].reason, SkipReason::ConsentDenied);
    assert_eq!(test_good_version(host), None);
    assert!(!detector_cache(&pixi).exists());
}

#[tokio::test]
async fn detectors_are_not_consulted_without_declared_names_or_dependencies() {
    let channel_dir = tempfile::tempdir().unwrap();
    write_channel(channel_dir.path()).await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let pixi = workspace_for(&channel, false);
    store_consent(&pixi, &channel, "allow");

    let workspace = locate(&pixi).await;
    let host = workspace.host();
    assert!(host.detector_results().is_empty());
    assert!(host.skipped_detectors().is_empty());
    assert!(host.detector_failures().is_empty());
    assert!(!detector_cache(&pixi).exists());
}

#[tokio::test]
async fn a_dependency_reference_runs_detector_without_a_declared_name() {
    let channel_dir = tempfile::tempdir().unwrap();
    let counter = channel_dir.path().join("runs");
    write_channel_with(channel_dir.path(), &[counting_detector(&counter, REPORT)]).await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let pixi = workspace_for(&channel, false);
    let mut manifest = fs_err::read_to_string(pixi.manifest_path()).unwrap();
    manifest.push_str("\n[dependencies]\nneeds-good = \"*\"\n");
    fs_err::write(pixi.manifest_path(), manifest).unwrap();
    store_consent(&pixi, &channel, "allow");

    let workspace = locate(&pixi).await;
    assert_eq!(
        test_good_version(workspace.host()).as_deref(),
        Some("1.2.3")
    );
    assert_eq!(runs(&counter), 1);
}

/// A friendly key declares a built-in name; a channel detector registered
/// for it runs and its answer replaces the built-in detection.
#[tokio::test]
async fn a_detector_for_a_built_in_name_runs_when_a_friendly_key_declares_it() {
    let channel_dir = tempfile::tempdir().unwrap();
    let cuda = Detector {
        name: "cuda-detect",
        names: &["__cuda"],
        script: DetectorScript::report(
            r#"{"version": 1, "virtual_packages": {"__cuda": {"version": "99.0"}}}"#,
        ),
    };
    write_channel_with(channel_dir.path(), &[cuda]).await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let pixi = workspace_declaring(&channel, Subdir::current().unwrap(), &[("__cuda", "12.0")]);
    store_consent(&pixi, &channel, "allow");

    let workspace = locate(&pixi).await;
    let host = workspace.host();
    assert!(
        host.detector_failures().is_empty(),
        "{:?}",
        host.detector_failures()
    );
    assert_eq!(host.detector_results().len(), 1);
    let cuda = host
        .platform()
        .unwrap()
        .customised_virtual_packages()
        .into_iter()
        .find(|package| package.name.as_normalized() == "__cuda")
        .expect("the detector's answer is part of the host platform");
    assert_eq!(cuda.version.to_string(), "99.0");
}

#[tokio::test]
async fn a_detector_for_an_unrelated_name_is_not_installed() {
    let channel_dir = tempfile::tempdir().unwrap();
    let counter = channel_dir.path().join("runs");
    write_channel_with(channel_dir.path(), &[counting_detector(&counter, REPORT)]).await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let pixi = workspace_declaring(
        &channel,
        Subdir::current().unwrap(),
        &[("__test_other", "1.0")],
    );
    store_consent(&pixi, &channel, "allow");

    let workspace = locate(&pixi).await;
    let host = workspace.host();
    assert!(host.detector_results().is_empty());
    assert!(host.detector_failures().is_empty());
    let skipped = host.skipped_detectors();
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].reason, SkipReason::NoWantedName);
    assert_eq!(runs(&counter), 0, "an unwanted detector must not execute");
    assert!(!detector_cache(&pixi).exists());
}

#[tokio::test]
async fn a_detector_that_exits_unsuccessfully_is_reported_and_its_name_absent() {
    let channel_dir = tempfile::tempdir().unwrap();
    let counter = channel_dir.path().join("runs");
    write_channel_with(
        channel_dir.path(),
        &[Detector::good(DetectorScript::failure(
            Some(&counter),
            "broken driver",
            3,
        ))],
    )
    .await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let pixi = workspace_for(&channel, true);
    store_consent(&pixi, &channel, "allow");

    let workspace = locate(&pixi).await;
    let host = workspace.host();
    assert!(host.detector_results().is_empty());
    let failures = host.detector_failures();
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(failures[0].detector.as_source(), DETECTOR);
    assert_eq!(
        failures[0].stderr.as_deref().map(str::trim),
        Some("broken driver")
    );
    assert_eq!(test_good_version(host), None);
    assert_eq!(runs(&counter), 1);
    let second = locate(&pixi).await;
    assert_eq!(runs(&counter), 2, "a failed detector must run again");
    assert_eq!(test_good_version(second.host()), None);
    assert_eq!(second.host().detector_failures().len(), 1);
}

#[tokio::test]
async fn a_detector_that_times_out_is_reported() {
    let channel_dir = tempfile::tempdir().unwrap();
    write_channel_with(
        channel_dir.path(),
        &[Detector::good(DetectorScript::delayed_report(REPORT))],
    )
    .await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let pixi = workspace_for(&channel, true);
    store_consent(&pixi, &channel, "allow");
    append_config(&pixi, "[virtual-package-detectors]\ntimeout-seconds = 1\n");

    let started = Instant::now();
    let workspace = locate(&pixi).await;
    let elapsed = started.elapsed();
    let host = workspace.host();
    let failures = host.detector_failures();
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(
        elapsed.as_secs() < 15,
        "the detector was not killed at the timeout: {elapsed:?}"
    );
    assert_eq!(test_good_version(host), None);
}

#[tokio::test]
async fn a_malformed_report_is_reported() {
    let channel_dir = tempfile::tempdir().unwrap();
    write_channel_with(
        channel_dir.path(),
        &[Detector::good(DetectorScript::report("not json"))],
    )
    .await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let pixi = workspace_for(&channel, true);
    store_consent(&pixi, &channel, "allow");

    let workspace = locate(&pixi).await;
    let host = workspace.host();
    let failures = host.detector_failures();
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(test_good_version(host), None);
}

#[tokio::test]
async fn a_report_omitting_a_registered_name_is_rejected() {
    let channel_dir = tempfile::tempdir().unwrap();
    write_channel_with(
        channel_dir.path(),
        &[Detector {
            name: DETECTOR,
            names: &["__test_good", "__test_more"],
            script: DetectorScript::report(REPORT),
        }],
    )
    .await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let pixi = workspace_for(&channel, true);
    store_consent(&pixi, &channel, "allow");

    let workspace = locate(&pixi).await;
    let host = workspace.host();
    let failures = host.detector_failures();
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(test_good_version(host), None);
}

#[tokio::test]
async fn a_report_can_declare_a_name_absent() {
    let channel_dir = tempfile::tempdir().unwrap();
    write_channel_with(
        channel_dir.path(),
        &[Detector {
            name: DETECTOR,
            names: &["__test_good", "__test_absent"],
            script: DetectorScript::report(
                r#"{"version": 1, "virtual_packages": {"__test_good": {"version": "1.2.3", "build_string": "h1"}, "__test_absent": null}}"#,
            ),
        }],
    )
    .await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let pixi = workspace_declaring(
        &channel,
        Subdir::current().unwrap(),
        &[("__test_good", "1.0"), ("__test_absent", "1.0")],
    );
    store_consent(&pixi, &channel, "allow");

    let workspace = locate(&pixi).await;
    let host = workspace.host();
    assert!(
        host.detector_failures().is_empty(),
        "{:?}",
        host.detector_failures()
    );
    let results = host.detector_results();
    assert_eq!(results.len(), 2, "{results:?}");
    let absent = results
        .iter()
        .find(|result| result.name.as_source() == "__test_absent")
        .unwrap();
    assert_eq!(absent.value, DetectedValue::Absent);
    let platform = host.platform().unwrap();
    let good = platform
        .customised_virtual_packages()
        .into_iter()
        .find(|package| package.name.as_source() == "__test_good")
        .unwrap();
    assert_eq!(good.version.to_string(), "1.2.3");
    assert_eq!(good.build_string, "h1");
    assert!(
        platform
            .customised_virtual_packages()
            .iter()
            .all(|package| package.name.as_source() != "__test_absent")
    );
}

#[tokio::test]
async fn one_channel_decision_allows_all_its_detectors() {
    let channel_dir = tempfile::tempdir().unwrap();
    write_channel_with(
        channel_dir.path(),
        &[
            Detector::default_report(),
            Detector {
                name: "other-detect",
                names: &["__test_other"],
                script: DetectorScript::report(
                    r#"{"version": 1, "virtual_packages": {"__test_other": {"version": "7"}}}"#,
                ),
            },
        ],
    )
    .await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let pixi = workspace_declaring(
        &channel,
        Subdir::current().unwrap(),
        &[("__test_good", "1.0"), ("__test_other", "1.0")],
    );
    store_consent(&pixi, &channel, "allow");

    let workspace = locate(&pixi).await;
    let host = workspace.host();
    assert!(
        host.detector_failures().is_empty(),
        "{:?}",
        host.detector_failures()
    );
    assert_eq!(host.detector_results().len(), 2);
    assert_eq!(test_good_version(host).as_deref(), Some("1.2.3"));
    let other = host
        .capabilities()
        .iter()
        .find(|package| package.name.as_normalized() == "__test_other")
        .unwrap();
    assert_eq!(other.version.to_string(), "7");
    assert!(host.skipped_detectors().is_empty());
}

#[tokio::test]
async fn a_cached_report_is_reused_on_the_next_detection() {
    let channel_dir = tempfile::tempdir().unwrap();
    let counter = channel_dir.path().join("runs");
    write_channel_with(channel_dir.path(), &[counting_detector(&counter, REPORT)]).await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let pixi = workspace_for(&channel, true);
    store_consent(&pixi, &channel, "allow");

    let first = locate(&pixi).await;
    assert_eq!(runs(&counter), 1);
    assert!(matches!(
        &first.host().detector_results()[0].source,
        DetectionSource::Detector {
            from_cache: false,
            ..
        }
    ));

    let second = locate(&pixi).await;
    assert_eq!(runs(&counter), 1, "the cached report must be reused");
    let host = second.host();
    assert_eq!(test_good_version(host).as_deref(), Some("1.2.3"));
    assert!(
        matches!(
            &host.detector_results()[0].source,
            DetectionSource::Detector {
                from_cache: true,
                ..
            }
        ),
        "{:?}",
        host.detector_results()
    );
}

#[tokio::test]
async fn a_zero_ttl_report_is_never_reused() {
    let channel_dir = tempfile::tempdir().unwrap();
    let counter = channel_dir.path().join("runs");
    let report = r#"{"version": 1, "virtual_packages": {"__test_good": {"version": "1.2.3"}}, "cache": {"ttl_seconds": 0}}"#;
    write_channel_with(channel_dir.path(), &[counting_detector(&counter, report)]).await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let pixi = workspace_for(&channel, true);
    store_consent(&pixi, &channel, "allow");

    locate(&pixi).await;
    let second = locate(&pixi).await;
    assert_eq!(runs(&counter), 2);
    assert_eq!(test_good_version(second.host()).as_deref(), Some("1.2.3"));
}

#[tokio::test]
async fn a_changed_watched_variable_invalidates_the_cache() {
    let channel_dir = tempfile::tempdir().unwrap();
    let counter = channel_dir.path().join("runs");
    let report = r#"{"version": 1, "virtual_packages": {"__test_good": {"version": "1.2.3"}}, "cache": {"watch_env": ["PIXI_TEST_DETECTOR_WATCHED"]}}"#;
    write_channel_with(channel_dir.path(), &[counting_detector(&counter, report)]).await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let pixi = workspace_for(&channel, true);
    store_consent(&pixi, &channel, "allow");

    temp_env::async_with_vars([("PIXI_TEST_DETECTOR_WATCHED", Some("one"))], async {
        locate(&pixi).await;
        locate(&pixi).await;
    })
    .await;
    assert_eq!(runs(&counter), 1, "an unchanged variable keeps the entry");

    temp_env::async_with_vars([("PIXI_TEST_DETECTOR_WATCHED", Some("two"))], async {
        let workspace = locate(&pixi).await;
        assert_eq!(
            test_good_version(workspace.host()).as_deref(),
            Some("1.2.3")
        );
    })
    .await;
    assert_eq!(runs(&counter), 2, "a changed variable re-runs the detector");

    temp_env::async_with_vars([("PIXI_TEST_DETECTOR_WATCHED", None::<&str>)], async {
        locate(&pixi).await;
    })
    .await;
    assert_eq!(runs(&counter), 3, "an unset variable re-runs the detector");
}

#[tokio::test]
async fn an_override_variable_wins_without_running_the_detector() {
    let channel_dir = tempfile::tempdir().unwrap();
    let counter = channel_dir.path().join("runs");
    write_channel_with(channel_dir.path(), &[counting_detector(&counter, REPORT)]).await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let pixi = workspace_for(&channel, true);
    store_consent(&pixi, &channel, "allow");

    temp_env::async_with_vars([("CONDA_OVERRIDE_TEST_GOOD", Some("9.9"))], async {
        let workspace = locate(&pixi).await;
        let host = workspace.host();
        assert!(host.detector_failures().is_empty());
        assert_eq!(test_good_version(host).as_deref(), Some("9.9"));
        let results = host.detector_results();
        assert_eq!(results.len(), 1, "{results:?}");
        assert_eq!(
            results[0].source,
            DetectionSource::Override {
                variable: "CONDA_OVERRIDE_TEST_GOOD".to_string()
            }
        );
        assert_eq!(host.skipped_detectors().len(), 1);
        assert_eq!(host.skipped_detectors()[0].reason, SkipReason::NoWantedName);
    })
    .await;
    assert_eq!(
        runs(&counter),
        0,
        "the detector must not run when overridden"
    );
    assert!(!detector_cache(&pixi).exists());
}

#[tokio::test]
async fn an_empty_override_variable_declares_the_name_absent() {
    let channel_dir = tempfile::tempdir().unwrap();
    let counter = channel_dir.path().join("runs");
    write_channel_with(channel_dir.path(), &[counting_detector(&counter, REPORT)]).await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let pixi = workspace_for(&channel, true);
    store_consent(&pixi, &channel, "allow");

    temp_env::async_with_vars([("CONDA_OVERRIDE_TEST_GOOD", Some(""))], async {
        let workspace = locate(&pixi).await;
        let host = workspace.host();
        assert_eq!(test_good_version(host), None);
        let results = host.detector_results();
        assert_eq!(results.len(), 1, "{results:?}");
        assert_eq!(results[0].value, DetectedValue::Absent);
    })
    .await;
    assert_eq!(runs(&counter), 0);
}

#[tokio::test]
async fn an_invalid_override_variable_fails_to_locate_the_workspace() {
    let channel_dir = tempfile::tempdir().unwrap();
    write_channel(channel_dir.path()).await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let pixi = workspace_for(&channel, true);
    store_consent(&pixi, &channel, "allow");

    temp_env::async_with_vars(
        [("CONDA_OVERRIDE_TEST_GOOD", Some("1.0=bad build!"))],
        async {
            assert!(try_locate(&pixi).await.is_err());
        },
    )
    .await;
}

#[tokio::test]
async fn a_platform_override_skips_detectors_and_names_the_override_variable() {
    let channel_dir = tempfile::tempdir().unwrap();
    let counter = channel_dir.path().join("runs");
    write_channel_with(channel_dir.path(), &[counting_detector(&counter, REPORT)]).await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let native = Subdir::current().unwrap();
    let foreign = if native == Subdir::Linux64 {
        Subdir::LinuxAarch64
    } else {
        Subdir::Linux64
    };
    let pixi = workspace_declaring(&channel, foreign, &[("__test_good", "1.0")]);
    store_consent(&pixi, &channel, "allow");

    temp_env::async_with_vars(
        [("PIXI_OVERRIDE_PLATFORM", Some(foreign.as_str()))],
        async {
            let workspace = locate(&pixi).await;
            let host = workspace.host();
            assert_eq!(host.subdir(), foreign);
            assert!(host.detector_results().is_empty());
            assert!(host.detector_failures().is_empty());
            let skipped = host.skipped_detectors();
            assert_eq!(skipped.len(), 1, "{skipped:?}");
            assert_eq!(
                skipped[0].reason,
                SkipReason::TargetIsNotHost {
                    override_variables: vec!["CONDA_OVERRIDE_TEST_GOOD".to_string()]
                }
            );
        },
    )
    .await;
    assert_eq!(runs(&counter), 0);

    // The override still supplies the name for the foreign platform.
    temp_env::async_with_vars(
        [
            ("PIXI_OVERRIDE_PLATFORM", Some(foreign.as_str())),
            ("CONDA_OVERRIDE_TEST_GOOD", Some("4.5")),
        ],
        async {
            let workspace = locate(&pixi).await;
            let host = workspace.host();
            assert_eq!(host.subdir(), foreign);
            assert_eq!(test_good_version(host).as_deref(), Some("4.5"));
        },
    )
    .await;
    assert_eq!(runs(&counter), 0);
}

#[tokio::test]
async fn all_detectors_run_when_asked_regardless_of_declared_names() {
    let channel_dir = tempfile::tempdir().unwrap();
    write_channel(channel_dir.path()).await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    // No platform declares `__test_good`, so locating does not run it ...
    let pixi = workspace_for(&channel, false);
    store_consent(&pixi, &channel, "allow");
    let workspace = locate(&pixi).await;
    assert!(workspace.host().detector_results().is_empty());

    // ... but `pixi info` and `pixi workspace platform list` ask for all.
    let host = workspace.detect_host_with_all_detectors().await.unwrap();
    assert!(
        host.detector_failures().is_empty(),
        "{:?}",
        host.detector_failures()
    );
    assert_eq!(host.detector_results().len(), 1);
    assert_eq!(test_good_version(&host).as_deref(), Some("1.2.3"));
    assert!(
        host.capabilities()
            .iter()
            .any(|package| package.name.as_source() == "__test_good")
    );
}

/// Loads the trusted consent fixture and project-local cache settings.
fn config_of(pixi: &PixiControl) -> Config {
    Config::load_with(
        pixi.workspace_path(),
        &GlobalConfigSource::File(pixi.workspace_path().join("trusted-config.toml")),
    )
    .merge_config(pixi.config_cli().into())
}

#[tokio::test]
async fn detection_for_specs_runs_only_detectors_the_repodata_references() {
    let channel_dir = tempfile::tempdir().unwrap();
    let counter = channel_dir.path().join("runs");
    write_channel_with(channel_dir.path(), &[counting_detector(&counter, REPORT)]).await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let pixi = workspace_for(&channel, false);
    store_consent(&pixi, &channel, "allow");
    let channels = vec![ChannelUrl::from(channel.clone())];
    let native = Subdir::current().unwrap();

    // `plain` references no virtual package, so the detector is not wanted.
    let detector = HostDetector::new(config_of(&pixi), Arc::new(DenyAll), None).unwrap();
    let plain = MatchSpec::from_str("plain", ParseStrictness::Strict).unwrap();
    let host = detector
        .detect_for_specs(&channels, native, &[plain])
        .await
        .unwrap();
    assert!(
        host.detector_results().is_empty(),
        "{:?}",
        host.detector_results()
    );
    assert_eq!(runs(&counter), 0);

    // `needs-good` depends on `__test_good`, so the detector runs.
    let needs_good = MatchSpec::from_str("needs-good", ParseStrictness::Strict).unwrap();
    let host = detector
        .detect_for_specs(&channels, native, &[needs_good])
        .await
        .unwrap();
    assert!(
        host.detector_failures().is_empty(),
        "{:?}",
        host.detector_failures()
    );
    assert_eq!(test_good_version(&host).as_deref(), Some("1.2.3"));
    assert_eq!(runs(&counter), 1);
}

#[tokio::test]
async fn detection_with_explicit_wanted_names_uses_the_configured_cache_root() {
    let channel_dir = tempfile::tempdir().unwrap();
    let counter = channel_dir.path().join("runs");
    write_channel_with(channel_dir.path(), &[counting_detector(&counter, REPORT)]).await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let pixi = workspace_for(&channel, false);
    store_consent(&pixi, &channel, "allow");
    let channels = vec![ChannelUrl::from(channel.clone())];

    let detector = HostDetector::new(config_of(&pixi), Arc::new(DenyAll), None).unwrap();
    let wanted = WantedNames::Only(["__test_good".parse().unwrap()].into_iter().collect());
    let host = detector.detect(&channels, wanted).await.unwrap();
    assert_eq!(test_good_version(&host).as_deref(), Some("1.2.3"));
    assert_eq!(runs(&counter), 1);
    let wanted = WantedNames::Only(["__test_good".parse().unwrap()].into_iter().collect());
    let cached = detector.detect(&channels, wanted).await.unwrap();
    assert_eq!(test_good_version(&cached).as_deref(), Some("1.2.3"));
    assert_eq!(runs(&counter), 1, "the configured cache is reused");

    fs_err::remove_dir_all(detector_cache(&pixi)).unwrap();
    let wanted = WantedNames::Only(["__test_good".parse().unwrap()].into_iter().collect());
    let refreshed = detector.detect(&channels, wanted).await.unwrap();
    assert_eq!(test_good_version(&refreshed).as_deref(), Some("1.2.3"));
    assert_eq!(
        runs(&counter),
        2,
        "removing the configured cache invalidates it"
    );
}

#[tokio::test]
async fn a_reboot_lifetime_is_cached_until_the_boot_session_changes() {
    let channel_dir = tempfile::tempdir().unwrap();
    let counter = channel_dir.path().join("runs");
    let report = r#"{"version": 1, "virtual_packages": {"__test_good": {"version": "1.2.3"}}, "cache": {"ttl_seconds": "REBOOT"}}"#;
    write_channel_with(channel_dir.path(), &[counting_detector(&counter, report)]).await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let pixi = workspace_for(&channel, true);
    store_consent(&pixi, &channel, "allow");

    locate(&pixi).await;
    let second = locate(&pixi).await;
    assert_eq!(
        runs(&counter),
        1,
        "a REBOOT result is reused within the boot session"
    );
    assert_eq!(test_good_version(second.host()).as_deref(), Some("1.2.3"));

    // The stored entry is bound to the boot session; forging another one
    // expires it.
    let results = detector_cache(&pixi).join("results");
    let entry = fs_err::read_dir(&results)
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .expect("the cached result exists")
        .path();
    let mut cached: serde_json::Value =
        serde_json::from_str(&fs_err::read_to_string(&entry).unwrap()).unwrap();
    cached["boot_id"] = serde_json::Value::String("00000000-0000-0000-0000-000000000000".into());
    fs_err::write(&entry, serde_json::to_string(&cached).unwrap()).unwrap();
    let third = locate(&pixi).await;
    assert_eq!(
        runs(&counter),
        2,
        "a different boot session re-runs the detector"
    );
    assert_eq!(test_good_version(third.host()).as_deref(), Some("1.2.3"));
}

#[tokio::test]
async fn a_changed_watched_path_invalidates_the_cache() {
    let channel_dir = tempfile::tempdir().unwrap();
    let counter = channel_dir.path().join("runs");
    let watched = channel_dir.path().join("watched-driver");
    let report = serde_json::json!({
        "version": 1,
        "virtual_packages": {"__test_good": {"version": "1.2.3"}},
        "cache": {"watch_paths": [watched]},
    })
    .to_string();
    write_channel_with(channel_dir.path(), &[counting_detector(&counter, &report)]).await;
    let channel = Url::from_directory_path(channel_dir.path()).unwrap();
    let pixi = workspace_for(&channel, true);
    store_consent(&pixi, &channel, "allow");

    locate(&pixi).await;
    locate(&pixi).await;
    assert_eq!(runs(&counter), 1, "an absent watched path stays absent");

    fs_err::write(&watched, "appeared").unwrap();
    locate(&pixi).await;
    assert_eq!(
        runs(&counter),
        2,
        "a watched path that appeared re-runs the detector"
    );
    locate(&pixi).await;
    assert_eq!(runs(&counter), 2);

    fs_err::remove_file(&watched).unwrap();
    locate(&pixi).await;
    assert_eq!(
        runs(&counter),
        3,
        "a watched path that disappeared re-runs the detector"
    );
}
