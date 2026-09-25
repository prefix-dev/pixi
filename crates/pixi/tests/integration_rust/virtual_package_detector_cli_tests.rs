//! Virtual package detectors driven through the `pixi` binary.
//!
//! Every test runs the built binary in a sandbox: a temporary home with its
//! own user configuration, `PIXI_HOME` and cache, with no terminal on
//! standard input, so an undecided detector is never prompted for.

use std::{
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

#[cfg(unix)]
use std::{ffi::OsString, os::unix::ffi::OsStringExt};

use rattler_conda_types::Subdir;
use url::Url;

use crate::virtual_package_detector_tests::{
    DETECTOR, Detector, DetectorScript, counting_detector, runs, write_channel_with,
};

const CONSUMER_COMMAND: &str = if cfg!(windows) {
    "needs-good.bat"
} else {
    "needs-good"
};

/// A temporary home the binary runs against.
struct Sandbox {
    home: tempfile::TempDir,
    channel_dir: tempfile::TempDir,
    channel: Url,
}

impl Sandbox {
    async fn new(detectors: &[Detector<'_>]) -> Self {
        let channel_dir = tempfile::tempdir().unwrap();
        write_channel_with(channel_dir.path(), detectors).await;
        let channel = Url::from_directory_path(channel_dir.path()).unwrap();
        let home = tempfile::tempdir().unwrap();
        fs_err::create_dir_all(home.path().join("work")).unwrap();
        let shared_config_dir = home.path().join("config").join("rattler");
        fs_err::create_dir_all(&shared_config_dir).unwrap();
        fs_err::write(shared_config_dir.join("config.toml"), "").unwrap();
        Self {
            home,
            channel_dir,
            channel,
        }
    }

    fn config_home(&self) -> PathBuf {
        self.home.path().join("config")
    }

    fn cache_dir(&self) -> PathBuf {
        self.home.path().join("cache")
    }

    fn pixi_home(&self) -> PathBuf {
        self.home.path().join("pixi-home")
    }

    /// A directory without a workspace to run commands from.
    fn work_dir(&self) -> PathBuf {
        self.home.path().join("work")
    }

    /// The canonical origin spelling used in a consent key.
    fn origin_key(&self) -> String {
        self.channel.as_str().trim_end_matches('/').to_string()
    }

    fn consent_key(&self) -> String {
        format!(
            "virtual-package-detectors.consent.\"{}\"",
            self.origin_key()
        )
    }

    /// Writes a workspace declaring `test_good` (or nothing) on the native
    /// platform, returning its directory.
    fn workspace(&self, declares_test_good: bool) -> PathBuf {
        let subdir = Subdir::current().unwrap();
        let platform = if declares_test_good {
            format!(r#"{{ platform = "{subdir}", test_good = "1.0" }}"#)
        } else {
            format!(r#""{subdir}""#)
        };
        let dir = self.home.path().join("workspace");
        fs_err::create_dir_all(&dir).unwrap();
        fs_err::write(
            dir.join("pixi.toml"),
            format!(
                r#"
[workspace]
name = "detectors"
channels = ["{}"]
platforms = [{platform}]
"#,
                self.channel
            ),
        )
        .unwrap();
        dir
    }

    /// Runs every allowed detector through `pixi info` from a workspace on
    /// `self.channel` and returns the `virtual_package_detectors` it reports.
    fn detectors_reported_by_info(&self) -> serde_json::Value {
        let dir = self.home.path().join("warm-up");
        fs_err::create_dir_all(&dir).unwrap();
        fs_err::write(
            dir.join("pixi.toml"),
            format!(
                r#"
[workspace]
name = "warm-up"
channels = ["{}"]
platforms = ["{}"]
"#,
                self.channel,
                Subdir::current().unwrap()
            ),
        )
        .unwrap();
        let info = self
            .pixi(&dir, &["info", "--json"])
            .expect_success("pixi info --json (warm-up)")
            .json();
        info["virtual_package_detectors"].clone()
    }

    fn command(&self, cwd: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pixi"));
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.home.path())
            .env("USERPROFILE", self.home.path())
            .env("APPDATA", self.config_home())
            .env("LOCALAPPDATA", self.home.path().join("local"))
            .env("XDG_CONFIG_HOME", self.config_home())
            .env("RATTLER_HOME", self.config_home().join("rattler"))
            .env("XDG_CACHE_HOME", self.home.path().join("xdg-cache"))
            .env("XDG_DATA_HOME", self.home.path().join("xdg-data"))
            .env("PIXI_HOME", self.pixi_home())
            .env("PIXI_CACHE_DIR", self.cache_dir())
            .env("NO_COLOR", "1")
            .current_dir(cwd)
            .stdin(Stdio::null());
        #[cfg(windows)]
        for name in [
            "SYSTEMROOT",
            "SYSTEMDRIVE",
            "COMSPEC",
            "PATHEXT",
            "TEMP",
            "TMP",
        ] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
    }

    fn pixi(&self, cwd: &Path, args: &[&str]) -> Run {
        let output = self.command(cwd).args(args).output().unwrap();
        Run::from(output)
    }
}

struct Run {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

impl From<Output> for Run {
    fn from(output: Output) -> Self {
        Self {
            status: output.status,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }
}

impl Run {
    fn expect_success(self, what: &str) -> Self {
        assert!(
            self.status.success(),
            "{what} failed with {}\nstdout:\n{}\nstderr:\n{}",
            self.status,
            self.stdout,
            self.stderr
        );
        self
    }

    fn expect_failure(self, what: &str) -> Self {
        assert!(
            !self.status.success(),
            "{what} unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
            self.stdout,
            self.stderr
        );
        self
    }

    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|error| {
            panic!(
                "stdout is not JSON ({error})\nstdout:\n{}\nstderr:\n{}",
                self.stdout, self.stderr
            )
        })
    }
}

fn entries(directory: &Path) -> Vec<String> {
    match fs_err::read_dir(directory) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn strings(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("not an array: {value}"))
        .iter()
        .map(|item| item.as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn install_info_and_platform_list_with_a_shared_consent() {
    let sandbox = Sandbox::new(&[Detector::default_report()]).await;
    let workspace = sandbox.workspace(true);

    let key = sandbox.consent_key();
    sandbox
        .pixi(&workspace, &["config", "set", "--shared", &key, "allow"])
        .expect_success("pixi config set --shared");

    sandbox
        .pixi(&workspace, &["install"])
        .expect_success("pixi install");

    let info = sandbox
        .pixi(&workspace, &["info", "--json"])
        .expect_success("pixi info --json")
        .json();
    let virtual_packages = strings(&info["virtual_packages"]);
    assert!(
        virtual_packages
            .iter()
            .any(|package| package.starts_with("__test_good=1.2.3")),
        "{virtual_packages:?}"
    );
    let detectors = info["virtual_package_detectors"].as_array().unwrap();
    assert_eq!(detectors.len(), 1, "{detectors:?}");
    assert_eq!(detectors[0]["detector"], DETECTOR);
    assert_eq!(detectors[0]["origin"], sandbox.channel.as_str());
    assert!(
        matches!(detectors[0]["state"].as_str(), Some("ran" | "cached")),
        "{detectors:?}"
    );
    assert_eq!(
        strings(&detectors[0]["virtual_packages"]),
        vec!["__test_good=1.2.3=0".to_string()]
    );

    let list = sandbox
        .pixi(&workspace, &["workspace", "platform", "list", "--json"])
        .expect_success("pixi workspace platform list --json")
        .json();
    let host = &list["platforms"][0];
    assert_eq!(host["is_autodetected"], true);
    let detected = strings(&host["detected_virtual_packages"]);
    assert!(
        detected
            .iter()
            .any(|package| package.starts_with("test_good=1.2.3")),
        "{detected:?}"
    );
    assert_eq!(
        list["platforms"][1]["detected_virtual_packages"],
        host["detected_virtual_packages"],
    );

    // A project-local denial takes precedence over the shared consent.
    sandbox
        .pixi(&workspace, &["config", "set", "--local", &key, "deny"])
        .expect_success("pixi config set --local deny");
    let info = sandbox
        .pixi(&workspace, &["info", "--json"])
        .expect_success("pixi info --json after local deny")
        .json();
    let detectors = info["virtual_package_detectors"].as_array().unwrap();
    assert_eq!(detectors[0]["state"], "skipped", "{detectors:?}");
    assert!(
        !strings(&info["virtual_packages"])
            .iter()
            .any(|package| package.starts_with("__test_good"))
    );
}

#[tokio::test]
async fn info_reports_undecided_allowed_and_denied_channel_states() {
    let sandbox = Sandbox::new(&[Detector::default_report()]).await;
    // Without a stored decision nothing can run, so `pixi info` learns about
    // the detector only through the workspace declaring one of its names.
    let workspace = sandbox.workspace(true);

    let info = sandbox
        .pixi(&workspace, &["info", "--json"])
        .expect_success("pixi info --json")
        .json();
    let detectors = info["virtual_package_detectors"].as_array().unwrap();
    assert_eq!(detectors.len(), 1, "{detectors:?}");
    assert_eq!(detectors[0]["state"], "skipped");

    let key = sandbox.consent_key();
    sandbox
        .pixi(&workspace, &["config", "set", "--shared", &key, "allow"])
        .expect_success("pixi config set --shared");

    let info = sandbox
        .pixi(&workspace, &["info", "--json"])
        .expect_success("pixi info --json")
        .json();
    let detectors = info["virtual_package_detectors"].as_array().unwrap();
    // The workspace probe ran the detector moments ago, so the run for
    // `pixi info` itself may serve the cached report.
    let state = detectors[0]["state"].as_str().unwrap();
    assert!(matches!(state, "ran" | "cached"), "{detectors:?}");
    assert!(
        strings(&info["virtual_packages"])
            .iter()
            .any(|package| package.starts_with("__test_good=1.2.3"))
    );

    sandbox
        .pixi(&workspace, &["config", "set", "--shared", &key, "deny"])
        .expect_success("pixi config set deny");
    let info = sandbox
        .pixi(&workspace, &["info", "--json"])
        .expect_success("pixi info --json")
        .json();
    let detectors = info["virtual_package_detectors"].as_array().unwrap();
    assert_eq!(detectors[0]["state"], "skipped", "{detectors:?}");
}

#[tokio::test]
async fn a_failed_detector_is_reported_by_info_without_capabilities() {
    let sandbox = Sandbox::new(&[Detector::good(DetectorScript::failure(
        None,
        "no driver",
        2,
    ))])
    .await;
    let workspace = sandbox.workspace(true);
    let key = sandbox.consent_key();
    sandbox
        .pixi(&workspace, &["config", "set", "--global", &key, "allow"])
        .expect_success("pixi config set --global");

    let detectors = sandbox.detectors_reported_by_info();
    assert_eq!(detectors[0]["state"], "failed", "{detectors:?}");

    let info = sandbox
        .pixi(&workspace, &["info", "--json"])
        .expect_success("pixi info --json")
        .json();
    let detectors = info["virtual_package_detectors"].as_array().unwrap();
    assert_eq!(detectors[0]["state"], "failed", "{detectors:?}");
    assert!(
        detectors[0]["detail"]
            .as_str()
            .unwrap()
            .contains("no driver"),
        "{detectors:?}"
    );
    assert!(
        !strings(&info["virtual_packages"])
            .iter()
            .any(|package| package.starts_with("__test_good"))
    );
}

#[tokio::test]
async fn config_set_refuses_pixi_keys_for_shared_and_accepts_them_for_global() {
    let sandbox = Sandbox::new(&[Detector::default_report()]).await;
    let work = sandbox.work_dir();

    sandbox
        .pixi(
            &work,
            &["config", "set", "--shared", "shell.change-ps1", "false"],
        )
        .expect_failure("pixi config set --shared shell.change-ps1");
    assert_eq!(
        fs_err::read_to_string(sandbox.config_home().join("rattler").join("config.toml")).unwrap(),
        ""
    );

    sandbox
        .pixi(
            &work,
            &["config", "set", "--global", "shell.change-ps1", "false"],
        )
        .expect_success("pixi config set --global shell.change-ps1");

    let key = sandbox.consent_key();
    sandbox
        .pixi(&work, &["config", "set", "--shared", &key, "deny"])
        .expect_success("pixi config set --shared consent");
    sandbox
        .pixi(
            &work,
            &[
                "config",
                "set",
                "--shared",
                "virtual-package-detectors.timeout-seconds",
                "5",
            ],
        )
        .expect_success("pixi config set --shared timeout");

    sandbox
        .pixi(
            &work,
            &[
                "config",
                "set",
                "--shared",
                "virtual-package-detectors.timeout-seconds",
                "301",
            ],
        )
        .expect_failure("timeout above the maximum");

    sandbox
        .pixi(&work, &["config", "set", "--shared", &key, "maybe"])
        .expect_failure("an invalid decision");

    let workspace = sandbox.workspace(true);
    sandbox
        .pixi(&work, &["config", "set", "--shared", &key, "allow"])
        .expect_success("allow detector before revoking consent");
    let allowed = sandbox
        .pixi(&workspace, &["info", "--json"])
        .expect_success("detect with shared consent")
        .json();
    assert!(
        strings(&allowed["virtual_packages"])
            .iter()
            .any(|package| package == "__test_good=1.2.3=0"),
        "{allowed}"
    );

    sandbox
        .pixi(
            &work,
            &["config", "unset", "--shared", &sandbox.consent_key()],
        )
        .expect_success("pixi config unset --shared consent");
    let revoked = sandbox
        .pixi(&workspace, &["info", "--json"])
        .expect_success("detect after revoking shared consent")
        .json();
    assert_eq!(revoked["virtual_package_detectors"][0]["state"], "skipped");
    assert!(
        !strings(&revoked["virtual_packages"])
            .iter()
            .any(|package| package.starts_with("__test_good=")),
        "{revoked}"
    );
}

#[tokio::test]
async fn exec_detects_explicit_virtual_specs_without_transitive_references() {
    let sandbox = Sandbox::new(&[Detector::default_report()]).await;
    let work = sandbox.work_dir();
    let channel = sandbox.channel.to_string();
    let key = sandbox.consent_key();
    sandbox
        .pixi(&work, &["config", "set", "--shared", &key, "allow"])
        .expect_success("allow detector");
    #[cfg(unix)]
    let command = ["echo", "explicit-virtual-spec"].as_slice();
    #[cfg(windows)]
    let command = ["cmd", "/d", "/c", "echo", "explicit-virtual-spec"].as_slice();
    let run = |requirement| {
        let mut args = vec![
            "exec",
            "--channel",
            &channel,
            "--spec",
            "plain",
            "--spec",
            requirement,
            "--",
        ];
        args.extend_from_slice(command);
        sandbox.pixi(&work, &args)
    };
    assert_eq!(
        run("__test_good >=1")
            .expect_success("solve explicit virtual requirement")
            .stdout
            .trim(),
        "explicit-virtual-spec"
    );
    run("__test_good >=9").expect_failure("reject unsatisfied virtual requirement");
}

#[cfg(unix)]
#[tokio::test]
async fn exec_preserves_native_environment_for_detectors_and_consumers() {
    let check = "test \"$PIXI_TEST_NATIVE\" = \"$(printf 'native-\\377')\" || exit 23";
    let sandbox = Sandbox::new(&[Detector::good(DetectorScript::unix_only(format!(
        "{check}\necho '{}'",
        crate::virtual_package_detector_tests::REPORT
    )))])
    .await;
    let work = sandbox.work_dir();
    let channel = sandbox.channel.to_string();
    let key = sandbox.consent_key();
    sandbox
        .pixi(&work, &["config", "set", "--shared", &key, "allow"])
        .expect_success("allow detector");
    let output = sandbox
        .command(&work)
        .env(
            "PIXI_TEST_NATIVE",
            OsString::from_vec(b"native-\xff".to_vec()),
        )
        .args([
            "exec",
            "--channel",
            &channel,
            "--spec",
            "needs-good",
            "--",
            "sh",
            "-c",
            &format!("{check}\nneeds-good"),
        ])
        .output()
        .unwrap();
    let run = Run::from(output).expect_success("exec with a native environment value");
    assert_eq!(run.stdout.trim(), "needs-good");
}

#[tokio::test]
async fn exec_solves_a_package_that_depends_on_a_detected_name() {
    let sandbox = Sandbox::new(&[Detector::default_report()]).await;
    let work = sandbox.work_dir();
    let channel = sandbox.channel.to_string();

    // Without a decision the solve cannot see `__test_good`.
    sandbox
        .pixi(
            &work,
            &["exec", "-c", &channel, "-s", "needs-good", CONSUMER_COMMAND],
        )
        .expect_failure("pixi exec without consent");

    let key = sandbox.consent_key();
    sandbox
        .pixi(&work, &["config", "set", "--shared", &key, "allow"])
        .expect_success("pixi config set --shared");
    let allowed = sandbox
        .pixi(
            &work,
            &["exec", "-c", &channel, "-s", "needs-good", CONSUMER_COMMAND],
        )
        .expect_success("pixi exec with consent");
    assert_eq!(allowed.stdout.trim(), "needs-good");

    // A package that references no detector name never runs the detector.
    let counter = sandbox.channel_dir.path().join("runs");
    let sandbox2 = Sandbox::new(&[counting_detector(
        &counter,
        crate::virtual_package_detector_tests::REPORT,
    )])
    .await;
    let key = sandbox2.consent_key();
    sandbox2
        .pixi(
            &sandbox2.work_dir(),
            &["config", "set", "--shared", &key, "allow"],
        )
        .expect_success("pixi config set --shared");
    let channel2 = sandbox2.channel.to_string();
    sandbox2
        .pixi(
            &sandbox2.work_dir(),
            &["exec", "-c", &channel2, "-s", "plain", "true"],
        )
        .expect_success("pixi exec plain");
    assert!(
        !counter.exists(),
        "the detector ran for a package that does not need it"
    );
}

#[tokio::test]
async fn clean_cache_removes_only_the_detector_cache() {
    let counter_dir = tempfile::tempdir().unwrap();
    let counter = counter_dir.path().join("runs");
    let sandbox = Sandbox::new(&[counting_detector(
        &counter,
        crate::virtual_package_detector_tests::REPORT,
    )])
    .await;
    let workspace = sandbox.workspace(true);
    let key = sandbox.consent_key();
    sandbox
        .pixi(&workspace, &["config", "set", "--shared", &key, "allow"])
        .expect_success("pixi config set --shared");
    sandbox
        .pixi(&workspace, &["install"])
        .expect_success("pixi install");
    assert_eq!(runs(&counter), 1);
    let cached = sandbox.detectors_reported_by_info();
    assert_eq!(cached[0]["state"], "cached", "{cached:?}");
    assert_eq!(
        runs(&counter),
        1,
        "the detector report is reused before cleaning"
    );

    let unrelated = sandbox.cache_dir().join("unrelated-cache");
    fs_err::create_dir_all(&unrelated).unwrap();
    let marker = unrelated.join("preserve");
    fs_err::write(&marker, "unrelated cache contents").unwrap();

    let before = entries(&sandbox.cache_dir());

    sandbox
        .pixi(
            &workspace,
            &["clean", "cache", "--virtual-package-detectors", "--yes"],
        )
        .expect_success("pixi clean cache --virtual-package-detectors");
    let after = entries(&sandbox.cache_dir());
    let mut others_before: Vec<_> = before
        .iter()
        .filter(|entry| *entry != pixi_consts::consts::VIRTUAL_PACKAGE_DETECTORS_CACHE_DIR)
        .cloned()
        .collect();
    let mut others_after: Vec<_> = after
        .iter()
        .filter(|entry| *entry != pixi_consts::consts::VIRTUAL_PACKAGE_DETECTORS_CACHE_DIR)
        .cloned()
        .collect();
    others_before.sort();
    others_after.sort();
    assert_eq!(others_before, others_after, "other caches were touched");
    assert_eq!(
        fs_err::read_to_string(&marker).unwrap(),
        "unrelated cache contents"
    );

    // The next detection installs and runs the detector again.
    let detectors = sandbox.detectors_reported_by_info();
    assert_eq!(detectors[0]["state"], "ran", "{detectors:?}");
    assert_eq!(
        runs(&counter),
        2,
        "the removed detector state cannot be reused"
    );
    assert_eq!(
        strings(&detectors[0]["virtual_packages"]),
        vec!["__test_good=1.2.3=0".to_string()]
    );
}

#[tokio::test]
async fn a_platform_override_does_not_execute_native_detectors() {
    let counter_dir = tempfile::tempdir().unwrap();
    let counter = counter_dir.path().join("runs");
    let sandbox = Sandbox::new(&[counting_detector(
        &counter,
        crate::virtual_package_detector_tests::REPORT,
    )])
    .await;
    let native = Subdir::current().unwrap();
    let foreign = if native == Subdir::Linux64 {
        Subdir::LinuxAarch64
    } else {
        Subdir::Linux64
    };
    let workspace = sandbox.home.path().join("workspace");
    fs_err::create_dir_all(&workspace).unwrap();
    fs_err::write(
        workspace.join("pixi.toml"),
        format!(
            r#"
[workspace]
name = "detectors"
channels = ["{}"]
platforms = [{{ platform = "{foreign}", test_good = "1.0" }}]
"#,
            sandbox.channel
        ),
    )
    .unwrap();
    let key = sandbox.consent_key();
    sandbox
        .pixi(&workspace, &["config", "set", "--shared", &key, "allow"])
        .expect_success("pixi config set --shared");

    let output = sandbox
        .command(&workspace)
        .env("PIXI_OVERRIDE_PLATFORM", foreign.as_str())
        .args(["info", "--json"])
        .output()
        .unwrap();
    let run = Run::from(output);
    assert_eq!(
        runs(&counter),
        0,
        "a platform override must not execute detectors"
    );
    let info = run.json();
    let detectors = info["virtual_package_detectors"].as_array().unwrap();
    assert_eq!(detectors[0]["state"], "skipped", "{detectors:?}");

    let output = sandbox
        .command(&workspace)
        .env("PIXI_OVERRIDE_PLATFORM", foreign.as_str())
        .env("CONDA_OVERRIDE_TEST_GOOD", "3.3")
        .args(["info", "--json"])
        .output()
        .unwrap();
    let info = Run::from(output)
        .expect_success("pixi info with overrides")
        .json();
    assert!(
        strings(&info["virtual_packages"])
            .iter()
            .any(|package| package.starts_with("__test_good=3.3")),
        "{}",
        info["virtual_packages"]
    );
    assert_eq!(runs(&counter), 0);
}

#[tokio::test]
async fn invalid_detector_overrides_are_errors_for_host_inspection() {
    let counter_dir = tempfile::tempdir().unwrap();
    let counter = counter_dir.path().join("runs");
    let sandbox = Sandbox::new(&[counting_detector(
        &counter,
        crate::virtual_package_detector_tests::REPORT,
    )])
    .await;
    let key = sandbox.consent_key();
    sandbox
        .pixi(
            &sandbox.work_dir(),
            &["config", "set", "--shared", &key, "allow"],
        )
        .expect_success("store shared consent");

    for declares_name in [true, false] {
        let workspace = sandbox.workspace(declares_name);
        for args in [
            vec!["info", "--json"],
            vec!["workspace", "platform", "list", "--json"],
            vec![
                "workspace",
                "platform",
                "add",
                "--auto-detect",
                "--no-install",
            ],
        ] {
            let output = sandbox
                .command(&workspace)
                .env("CONDA_OVERRIDE_TEST_GOOD", "not a version")
                .args(&args)
                .output()
                .unwrap();
            let run = Run::from(output).expect_failure("invalid detector override");
            assert!(
                run.stderr.contains("CONDA_OVERRIDE_TEST_GOOD"),
                "{args:?}: {}",
                run.stderr
            );
        }
    }
    assert_eq!(runs(&counter), 0);
}

#[tokio::test]
async fn foreign_exec_and_global_targets_preserve_overrides_without_execution() {
    let counter_dir = tempfile::tempdir().unwrap();
    let counter = counter_dir.path().join("runs");
    let sandbox = Sandbox::new(&[counting_detector(
        &counter,
        crate::virtual_package_detector_tests::REPORT,
    )])
    .await;
    let native = Subdir::current().unwrap();
    let foreign = if native == Subdir::Linux64 {
        Subdir::LinuxAarch64
    } else {
        Subdir::Linux64
    };
    let foreign_dir = sandbox.channel_dir.path().join(foreign.as_str());
    fs_err::create_dir_all(&foreign_dir).unwrap();
    fs_err::write(
        foreign_dir.join("repodata.json"),
        serde_json::json!({
            "info": { "subdir": foreign.as_str() },
            "packages": {},
            "packages.conda": {},
            "repodata_version": 1,
        })
        .to_string(),
    )
    .unwrap();
    let work = sandbox.work_dir();
    let key = sandbox.consent_key();
    sandbox
        .pixi(&work, &["config", "set", "--shared", &key, "allow"])
        .expect_success("store shared consent");
    let channel = sandbox.channel.to_string();
    for args in [
        vec![
            "exec",
            "--platform",
            foreign.as_str(),
            "-c",
            &channel,
            "-s",
            "needs-good",
            CONSUMER_COMMAND,
        ],
        vec![
            "global",
            "install",
            "--platform",
            foreign.as_str(),
            "-c",
            &channel,
            "needs-good",
        ],
    ] {
        let run = sandbox
            .pixi(&work, &args)
            .expect_failure("foreign target without override");
        assert!(
            run.stderr.contains("CONDA_OVERRIDE_TEST_GOOD"),
            "{args:?}: {}",
            run.stderr
        );
        let output = sandbox
            .command(&work)
            .env("CONDA_OVERRIDE_TEST_GOOD", "not a version")
            .args(&args)
            .output()
            .unwrap();
        let run = Run::from(output).expect_failure("foreign target with invalid override");
        assert!(
            run.stderr.contains("CONDA_OVERRIDE_TEST_GOOD"),
            "{args:?}: {}",
            run.stderr
        );
        let output = sandbox
            .command(&work)
            .env("CONDA_OVERRIDE_TEST_GOOD", "3.3")
            .args(&args)
            .output()
            .unwrap();
        Run::from(output).expect_success("foreign target with override");
    }
    assert_eq!(runs(&counter), 0);
}

#[tokio::test]
async fn global_install_solves_with_a_detected_name() {
    let sandbox = Sandbox::new(&[Detector::default_report()]).await;
    let work = sandbox.work_dir();
    let channel = sandbox.channel.to_string();

    sandbox
        .pixi(&work, &["global", "install", "-c", &channel, "needs-good"])
        .expect_failure("pixi global install without consent");

    let key = sandbox.consent_key();
    sandbox
        .pixi(&work, &["config", "set", "--shared", &key, "allow"])
        .expect_success("pixi config set --shared");
    sandbox
        .pixi(&work, &["global", "install", "-c", &channel, "needs-good"])
        .expect_success("pixi global install with consent");
    let exposed = sandbox
        .pixi_home()
        .join("bin")
        .join("needs-good")
        .with_extension(std::env::consts::EXE_EXTENSION);
    assert!(
        exposed.exists(),
        "{:?}",
        entries(&sandbox.pixi_home().join("bin"))
    );
}

#[tokio::test]
async fn install_succeeds_the_first_time_a_detector_environment_is_created() {
    let sandbox = Sandbox::new(&[Detector::default_report()]).await;
    let workspace = sandbox.workspace(true);
    let key = sandbox.consent_key();
    sandbox
        .pixi(&workspace, &["config", "set", "--shared", &key, "allow"])
        .expect_success("pixi config set --shared");

    sandbox
        .pixi(&workspace, &["install"])
        .expect_success("the first pixi install after consent");
}

#[tokio::test]
async fn exec_succeeds_the_first_time_a_detector_environment_is_created() {
    let sandbox = Sandbox::new(&[Detector::default_report()]).await;
    let work = sandbox.work_dir();
    let channel = sandbox.channel.to_string();
    let key = sandbox.consent_key();
    sandbox
        .pixi(&work, &["config", "set", "--shared", &key, "allow"])
        .expect_success("pixi config set --shared");

    let exec = sandbox.pixi(
        &work,
        &["exec", "-c", &channel, "-s", "needs-good", CONSUMER_COMMAND],
    );
    assert!(
        exec.status.success(),
        "the first pixi exec after consent failed with {}\nstderr:\n{}",
        exec.status,
        exec.stderr
    );
    assert_eq!(exec.stdout.trim(), "needs-good");
}

/// A trailing slash identifies the same consent origin for allow and revoke.
#[tokio::test]
async fn config_set_accepts_a_file_origin_with_a_trailing_slash() {
    let sandbox = Sandbox::new(&[Detector::default_report()]).await;
    let work = sandbox.work_dir();
    let workspace = sandbox.workspace(true);
    let slashed = format!("virtual-package-detectors.consent.\"{}\"", sandbox.channel);
    assert!(sandbox.channel.as_str().ends_with('/'));
    sandbox
        .pixi(&work, &["config", "set", "--shared", &slashed, "allow"])
        .expect_success("pixi config set --shared with a trailing slash");
    let allowed = sandbox
        .pixi(&workspace, &["info", "--json"])
        .expect_success("detect with a trailing-slash consent origin")
        .json();
    assert!(
        strings(&allowed["virtual_packages"])
            .iter()
            .any(|package| package == "__test_good=1.2.3=0"),
        "{allowed}"
    );
    sandbox
        .pixi(&work, &["config", "unset", "--shared", &slashed])
        .expect_success("pixi config unset --shared with a trailing slash");
    let revoked = sandbox
        .pixi(&workspace, &["info", "--json"])
        .expect_success("detect after revoking a trailing-slash consent origin")
        .json();
    assert_eq!(revoked["virtual_package_detectors"][0]["state"], "skipped");
    assert!(
        !strings(&revoked["virtual_packages"])
            .iter()
            .any(|package| package.starts_with("__test_good=")),
        "{revoked}"
    );
}

#[tokio::test]
async fn offline_workspace_commands_do_not_probe_http_channels() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let channel = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let server_requests = requests.clone();
    let server_stop = stop.clone();
    let server = thread::spawn(move || {
        while !server_stop.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut buffer = [0; 4096];
                    let _ = stream.read(&mut buffer);
                    server_requests.fetch_add(1, Ordering::SeqCst);
                    let _ = stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("HTTP listener failed: {error}"),
            }
        }
    });
    let commands: &[&[&str]] = &[
        &["install", "--offline"],
        &["shell", "--offline"],
        &["shell-hook", "--shell", "bash", "--offline"],
        &["reinstall", "--offline"],
        &["update", "--offline"],
        &["upgrade", "--offline"],
        &[
            "workspace",
            "export",
            "conda-explicit-spec",
            "exported",
            "--offline",
        ],
        &["search", "plain", "--offline"],
        &[
            "import",
            "environment.yml",
            "--format",
            "conda-env",
            "--offline",
        ],
        &["publish", "--dry-run", "--offline"],
    ];
    let mut traffic = Vec::new();
    for args in commands {
        let mut sandbox = Sandbox::new(&[]).await;
        sandbox.channel = channel.clone();
        let workspace = sandbox.workspace(true);
        fs_err::write(
            workspace.join("environment.yml"),
            "name: imported\ndependencies: []\n",
        )
        .unwrap();
        let before = requests.load(Ordering::SeqCst);
        let run = sandbox.pixi(&workspace, args);
        let after = requests.load(Ordering::SeqCst);
        if after != before {
            traffic.push(format!(
                "{args:?}: {} requests\n{}",
                after - before,
                run.stderr
            ));
        }
        let online_args = args
            .iter()
            .copied()
            .filter(|arg| *arg != "--offline")
            .collect::<Vec<_>>();
        let online = sandbox.pixi(&workspace, &online_args);
        if requests.load(Ordering::SeqCst) == after {
            traffic.push(format!(
                "{online_args:?}: online control never reached the channel\n{}",
                online.stderr
            ));
        }
    }
    stop.store(true, Ordering::SeqCst);
    server.join().unwrap();
    assert!(
        traffic.is_empty(),
        "offline request regression:\n{}",
        traffic.join("\n")
    );
}

#[tokio::test]
async fn conda_script_lock_includes_detected_dependency_capabilities() {
    let sandbox = Sandbox::new(&[Detector::default_report()]).await;
    let work = sandbox.work_dir();
    let key = sandbox.consent_key();
    sandbox
        .pixi(&work, &["config", "set", "--shared", &key, "allow"])
        .expect_success("allow script detector");
    let script = work.join("detected.sh");
    fs_err::write(
        &script,
        format!(
            "# /// conda-script\n# channels = [\"{}\"]\n# entrypoint = \"needs-good\"\n# [dependencies]\n# needs-good = \"*\"\n# /// end-conda-script\n",
            sandbox.channel
        ),
    )
    .unwrap();
    sandbox
        .pixi(&work, &["lock", "--script", script.to_str().unwrap()])
        .expect_success("lock script requiring a detected capability");
    let run = sandbox
        .pixi(
            &work,
            &[
                "run",
                "--experimental",
                "--locked",
                "--script",
                script.to_str().unwrap(),
            ],
        )
        .expect_success("run detector-aware script lock");
    assert_eq!(run.stdout.trim(), "needs-good");
}

async fn info_unix_packages(report: &str) -> Vec<String> {
    let sandbox = Sandbox::new(&[Detector {
        name: "unix-detect",
        names: &["__unix"],
        script: DetectorScript::report(report),
    }])
    .await;
    let workspace = sandbox.workspace(false);
    let key = sandbox.consent_key();
    sandbox
        .pixi(&workspace, &["config", "set", "--shared", &key, "allow"])
        .expect_success("allow builtin replacement detector");
    let info = sandbox
        .pixi(&workspace, &["info", "--json"])
        .expect_success("info with builtin replacement detector")
        .json();
    assert!(
        matches!(
            info["virtual_package_detectors"][0]["state"].as_str(),
            Some("ran" | "cached")
        ),
        "{info}"
    );
    strings(&info["virtual_packages"])
        .into_iter()
        .filter(|package| package.starts_with("__unix="))
        .collect()
}

#[tokio::test]
async fn info_replaces_builtin_virtual_packages() {
    let packages = info_unix_packages(
        r#"{"version":1,"virtual_packages":{"__unix":{"version":"99","build_string":"detected"}}}"#,
    )
    .await;
    assert_eq!(packages, ["__unix=99=detected"]);
}

#[tokio::test]
async fn info_removes_builtin_virtual_packages_reported_absent() {
    let packages = info_unix_packages(r#"{"version":1,"virtual_packages":{"__unix":null}}"#).await;
    assert!(packages.is_empty(), "{packages:?}");
}

#[tokio::test]
async fn direct_manifest_keys_match_canonical_detector_names_and_exact_builds() {
    async fn sandbox_with_report(build_string: &str) -> (Sandbox, PathBuf) {
        let report = format!(
            r#"{{"version":1,"virtual_packages":{{"__site_service":{{"version":"3","build_string":"{build_string}"}}}}}}"#
        );
        let sandbox = Sandbox::new(&[Detector {
            name: "site-detect",
            names: &["__site_service"],
            script: DetectorScript::report(&report),
        }])
        .await;
        let workspace = sandbox.home.path().join("site-workspace");
        fs_err::create_dir_all(&workspace).unwrap();
        fs_err::write(
            workspace.join("pixi.toml"),
            format!(
                r#"
[workspace]
name = "site-service"
channels = ["{}"]
platforms = [{{ name = "site", platform = "{}", site_service = "2=h1" }}]

[target.site.tasks]
check = "echo site service available"
"#,
                sandbox.channel,
                Subdir::current().unwrap()
            ),
        )
        .unwrap();
        let key = sandbox.consent_key();
        sandbox
            .pixi(&workspace, &["config", "set", "--shared", &key, "allow"])
            .expect_success("allow site detector");
        (sandbox, workspace)
    }

    let (matching, workspace) = sandbox_with_report("h1").await;
    matching
        .pixi(&workspace, &["run", "check"])
        .expect_success("select the platform with a matching capability build");
    let info = matching
        .pixi(&workspace, &["info", "--json"])
        .expect_success("inspect the detected arbitrary capability")
        .json();
    assert!(
        strings(&info["virtual_packages"])
            .iter()
            .any(|package| package == "__site_service=3=h1"),
        "{}",
        info["virtual_packages"]
    );

    let (mismatching, workspace) = sandbox_with_report("h2").await;
    let info = mismatching
        .pixi(&workspace, &["info", "--json"])
        .expect_success("inspect the mismatching detected capability build")
        .json();
    assert!(
        strings(&info["virtual_packages"])
            .iter()
            .any(|package| package == "__site_service=3=h2"),
        "{info}"
    );
    let listed = mismatching
        .pixi(&workspace, &["workspace", "platform", "list", "--json"])
        .expect_success("inspect the declared exact capability build")
        .json();
    let site = listed["platforms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|platform| platform["name"] == "site")
        .expect("the declared site platform is listed");
    assert!(
        strings(&site["virtual_packages"])
            .iter()
            .any(|package| package == "site_service=2=h1"),
        "{listed}"
    );
}

#[tokio::test]
async fn platform_commands_round_trip_friendly_and_canonical_virtual_packages() {
    let sandbox = Sandbox::new(&[]).await;
    let workspace = sandbox.workspace(false);
    let subdir = Subdir::current().unwrap();
    let named_platform = format!("custom={subdir}");

    sandbox
        .pixi(
            &workspace,
            &[
                "workspace",
                "platform",
                "add",
                &named_platform,
                "site_service=2=h1",
                "__amdgpu=0",
                "__name=1=h1",
                "__platform=2=h2",
                "__windows=3=h3",
                "__macos=4=h4",
                "--virtual-package",
                "kernel_api=linux-64",
                "--no-install",
            ],
        )
        .expect_success("add friendly and canonical virtual packages");

    let listed = sandbox
        .pixi(&workspace, &["workspace", "platform", "list", "--json"])
        .expect_success("list the added platform")
        .json();
    let added = listed["platforms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|platform| platform["name"] == "custom")
        .expect("custom platform should be listed");
    let packages = strings(&added["virtual_packages"]);
    assert!(
        packages
            .iter()
            .any(|package| package == "site_service=2=h1")
    );
    assert!(packages.iter().any(|package| package == "amdgpu"));
    assert!(
        packages
            .iter()
            .any(|package| package == "kernel_api=linux-64")
    );
    for package in [
        "__name=1=h1",
        "__platform=2=h2",
        "__windows=3=h3",
        "__macos=4=h4",
    ] {
        assert!(packages.iter().any(|listed| listed == package), "{listed}");
    }

    sandbox
        .pixi(
            &workspace,
            &[
                "workspace",
                "platform",
                "edit",
                "custom",
                "__site_service=3=h2",
                "--remove-virtual-package",
                "__amdgpu",
                "--no-install",
            ],
        )
        .expect_success("edit and remove canonical virtual packages");

    let listed = sandbox
        .pixi(&workspace, &["workspace", "platform", "list", "--json"])
        .expect_success("list the edited platform")
        .json();
    let edited = listed["platforms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|platform| platform["name"] == "custom")
        .expect("custom platform should still be listed");
    let packages = strings(&edited["virtual_packages"]);
    assert!(
        packages
            .iter()
            .any(|package| package == "site_service=3=h2")
    );
    assert!(!packages.iter().any(|package| package == "amdgpu"));
}
