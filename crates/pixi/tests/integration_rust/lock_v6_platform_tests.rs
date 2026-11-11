//! Pre-v7 lockfile lookup for migrated platforms.
//!
//! A v6 lockfile keys its platform rows by the bare conda subdir (`osx-arm64`)
//! and records no virtual packages -- that format predates them. When the
//! workspace migrates a `[system-requirements]` into a rich platform, that
//! platform's name (`osx-arm64-macos-12-0`) no longer matches the lock's subdir
//! key, and a raw `lock_file.platform(name)` lookup misses the row -- the bug
//! that made `pixi list`/`pixi tree` report an empty environment on the
//! affected machine. These exercise the resolution wiring against a real parsed
//! lock and a real workspace, with no solve and no network.

use crate::common::PixiControl;
use pixi_core::lock_file::resolve_lock_platform_for;
use pixi_manifest::HasWorkspaceManifest;
use rattler_lock::{LockFile, LockedPackage};

/// Workspace whose `[system-requirements]` migrates `osx-arm64` into the rich
/// platform `osx-arm64-macos-12-0`.
const MANIFEST: &str = r#"
[workspace]
name = "sysreq-v6"
channels = ["conda-forge"]
platforms = ["linux-64", "osx-arm64"]

[dependencies]
dummy = "*"

[system-requirements]
macos = "12.0"
"#;

/// A pre-v7 lockfile keyed by the bare subdir, with one conda package per
/// platform so the read-only commands have something to report.
const V6_LOCK: &str = r#"version: 6
environments:
  default:
    channels:
    - url: https://conda.anaconda.org/conda-forge/
    packages:
      osx-arm64:
      - conda: https://conda.anaconda.org/conda-forge/osx-arm64/dummy-1.0-h0.conda
      linux-64:
      - conda: https://conda.anaconda.org/conda-forge/linux-64/dummy-1.0-h0.conda
packages:
- conda: https://conda.anaconda.org/conda-forge/osx-arm64/dummy-1.0-h0.conda
  sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
  name: dummy
  version: '1.0'
  build: h0
  build_number: 0
  subdir: osx-arm64
  depends:
  - __osx >=11.0
  license: MIT
  size: 1234
  timestamp: 1700000000000
- conda: https://conda.anaconda.org/conda-forge/linux-64/dummy-1.0-h0.conda
  sha256: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
  name: dummy
  version: '1.0'
  build: h0
  build_number: 0
  subdir: linux-64
  license: MIT
  size: 1234
  timestamp: 1700000000000
"#;

/// Sets up the migrated workspace and writes the pre-v7 lockfile next to it.
fn workspace_with_v6_lock() -> PixiControl {
    let pixi = PixiControl::from_manifest(MANIFEST).unwrap();
    fs_err::write(pixi.workspace_path().join("pixi.lock"), V6_LOCK).unwrap();
    pixi
}

/// `default` environment's `dummy` package is reported for the resolved
/// platform's subdir-keyed v6 row.
fn lock_reports_dummy(lock: &LockFile, resolved: rattler_lock::Platform<'_>) -> bool {
    lock.environment("default")
        .expect("the v6 lock declares a default environment")
        .packages(resolved)
        .into_iter()
        .flatten()
        .filter_map(LockedPackage::as_conda)
        .any(|package| package.name().as_normalized() == "dummy")
}

/// Naming the migrated rich platform directly resolves to its packages, which
/// still live under the bare `osx-arm64` key in the v6 lock.
#[tokio::test]
async fn v6_lock_resolves_for_explicit_migrated_platform() {
    let pixi = workspace_with_v6_lock();
    let workspace = pixi.workspace().unwrap();
    let environment = workspace
        .environment("default")
        .expect("default environment");
    let platform = environment
        .workspace_manifest()
        .workspace
        .platforms
        .iter()
        .find(|p| p.name().as_str() == "osx-arm64-macos-12-0")
        .expect("macos system-requirement migrates osx-arm64 into a rich platform");

    let lock = pixi.lock_file().await.unwrap();
    let resolved = resolve_lock_platform_for(&lock, platform)
        .expect("the subdir-keyed pre-v7 row must resolve for the migrated platform");

    assert!(
        lock_reports_dummy(&lock, resolved),
        "dummy must be reported for the explicitly named migrated platform",
    );
}

/// The reported failure: on an arm Mac the environment's best platform is
/// `osx-arm64-macos-12-0`, and reading the v6 lock by that name used to miss
/// the subdir-keyed row and report an empty environment.
#[tokio::test]
async fn v6_lock_resolves_for_current_machine() {
    let pixi = workspace_with_v6_lock();

    // Mock an arm Mac: the host subdir plus the `__osx` the migrated platform
    // requires, so `best_declared_platform` selects `osx-arm64-macos-12-0`.
    temp_env::async_with_vars(
        [
            ("PIXI_OVERRIDE_PLATFORM", Some("osx-arm64")),
            ("CONDA_OVERRIDE_OSX", Some("12.0")),
        ],
        async {
            let workspace = pixi.workspace().unwrap();
            let environment = workspace
                .environment("default")
                .expect("default environment");
            let platform = environment
                .best_declared_platform()
                .expect("the mocked arm Mac must support the migrated platform");
            assert_eq!(
                platform.name().as_str(),
                "osx-arm64-macos-12-0",
                "the mocked machine's best platform is the migrated one",
            );

            let lock = pixi.lock_file().await.unwrap();
            let resolved = resolve_lock_platform_for(&lock, platform)
                .expect("the subdir-keyed pre-v7 row must resolve for the migrated platform");

            assert!(
                lock_reports_dummy(&lock, resolved),
                "dummy must be reported for the current machine's migrated platform",
            );
        },
    )
    .await;
}
