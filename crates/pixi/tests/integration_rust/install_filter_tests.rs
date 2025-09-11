use pixi_core::{InstallFilter, UpdateLockFileOptions, lock_file::PackageFilterNames};
use pixi_utils::prefix::Prefix as CondaPrefix;
use rattler_conda_types::{PackageName, Platform};

use crate::common::{
    PixiControl,
    package_database::{Package, PackageDatabase},
};

/// Helper to check if a conda package is installed in a prefix
async fn is_conda_package_installed(prefix_path: &std::path::Path, package_name: &str) -> bool {
    let conda_prefix = CondaPrefix::new(prefix_path.to_path_buf());
    conda_prefix
        .find_designated_package(&PackageName::try_from(package_name).unwrap())
        .await
        .is_ok()
}

// Build a simple package graph for tests:
// a -> {b, c}; c -> {d}; e (independent)
async fn setup_simple_graph_project() -> (PixiControl, crate::common::package_database::LocalChannel)
{
    let mut db = PackageDatabase::default();

    // Leaf nodes
    db.add_package(Package::build("b", "1").finish());
    db.add_package(Package::build("d", "1").finish());
    db.add_package(Package::build("e", "1").finish());

    // c depends on d
    db.add_package(Package::build("c", "1").with_dependency("d >=1").finish());

    // a depends on b and c
    db.add_package(
        Package::build("a", "1")
            .with_dependency("b >=1")
            .with_dependency("c >=1")
            .finish(),
    );

    let channel = db.into_channel().await.unwrap();

    let platform = Platform::current();
    let manifest = format!(
        r#"
        [project]
        name = "install-subset"
        channels = ["{channel}"]
        platforms = ["{platform}"]

        [dependencies]
        a = "*"
        e = "*"
        "#,
        channel = channel.url(),
    );

    (
        PixiControl::from_manifest(&manifest).expect("cannot instantiate pixi project"),
        channel,
    )
}

#[tokio::test]
async fn install_filter_skip_direct_soft_exclusion() {
    let (pixi, _channel) = setup_simple_graph_project().await;

    // Ensure lockfile exists
    pixi.update_lock_file().await.unwrap();

    // Build derived data and workspace env
    let workspace = pixi.workspace().unwrap();
    let (derived, _) = workspace
        .update_lock_file(UpdateLockFileOptions::default())
        .await
        .unwrap();
    let env = workspace.environment("default").unwrap();

    // Skip only the node `a` but traverse through its deps
    let filter = InstallFilter::new().skip_direct(vec!["a".to_string()]);
    let skipped = PackageFilterNames::new(
        &filter,
        derived.lock_file.environment(env.name().as_str()).unwrap(),
        env.best_platform(),
    )
    .unwrap()
    .ignored;

    // Only `a` should be skipped; b, c, d remain required via passthrough; e
    // remains
    assert_eq!(skipped, vec!["a".to_string()]);
}

#[tokio::test]
async fn install_filter_skip_with_deps_hard_exclusion() {
    let (pixi, _channel) = setup_simple_graph_project().await;
    pixi.update_lock_file().await.unwrap();

    let workspace = pixi.workspace().unwrap();
    let (derived, _) = workspace
        .update_lock_file(UpdateLockFileOptions::default())
        .await
        .unwrap();
    let env = workspace.environment("default").unwrap();

    // Hard skip `a` including its dependency subtree
    let filter = InstallFilter::new().skip_with_deps(vec!["a".to_string()]);
    let skipped = PackageFilterNames::new(
        &filter,
        derived.lock_file.environment(env.name().as_str()).unwrap(),
        env.best_platform(),
    )
    .unwrap()
    .ignored;

    // a, b, c, d are excluded; e remains as an independent root
    assert_eq!(
        skipped,
        vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ]
    );
}

#[tokio::test]
async fn install_filter_target_package_zoom_in() {
    let (pixi, _channel) = setup_simple_graph_project().await;
    pixi.update_lock_file().await.unwrap();

    let workspace = pixi.workspace().unwrap();
    let env = workspace.environment("default").unwrap();

    // Use derived.get_skipped_package_names with target mode
    let (derived, _) = workspace
        .update_lock_file(UpdateLockFileOptions::default())
        .await
        .unwrap();
    let filter = InstallFilter::new().target_packages(vec!["a".to_string()]);
    let skipped = PackageFilterNames::new(
        &filter,
        derived.lock_file.environment(env.name().as_str()).unwrap(),
        env.best_platform(),
    )
    .unwrap()
    .ignored;
    assert_eq!(skipped, vec!["e".to_string()]);
}

#[tokio::test]
async fn install_filter_target_with_skip_with_deps_stop() {
    let (pixi, _channel) = setup_simple_graph_project().await;
    pixi.update_lock_file().await.unwrap();

    let workspace = pixi.workspace().unwrap();
    let env = workspace.environment("default").unwrap();

    // Target a, but hard-skip c subtree: expect skipped c,d,e
    let (derived, _) = workspace
        .update_lock_file(UpdateLockFileOptions::default())
        .await
        .unwrap();
    let filter = InstallFilter::new()
        .target_packages(vec!["a".to_string()])
        .skip_with_deps(vec!["c".to_string()]);
    let skipped = PackageFilterNames::new(
        &filter,
        derived.lock_file.environment(env.name().as_str()).unwrap(),
        env.best_platform(),
    )
    .unwrap()
    .ignored;
    assert_eq!(skipped, vec!["c", "d", "e"]);
}

// Test to test the actual installation and if this makes sense
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn install_subset_e2e_skip_with_deps() {
    use std::path::{Path, PathBuf};

    use url::Url;

    // manifest with dependent packages: dummy-g depends on dummy-b
    let platform = Platform::current();
    let channel_path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/data/channels/channels/dummy_channel_1");
    let channel_path = fs_err::canonicalize(channel_path).expect("canonicalize channel path");
    let channel_url = Url::from_directory_path(&channel_path).expect("valid file url");
    let manifest = format!(
        r#"
        [project]
        name = "e2e-install-filter-hard-skip"
        channels = ["{channel}"]
        platforms = ["{platform}"]

        [dependencies]
        dummy-g = "*"
        dummy-a = "*"
        "#,
        channel = channel_url,
    );

    let pixi = PixiControl::from_manifest(&manifest).expect("cannot instantiate pixi project");
    pixi.update_lock_file().await.unwrap();

    // Hard-skip dummy-g subtree: expect dummy-g absent, and since dummy-g depends
    // on dummy-b, dummy-b is also absent
    pixi.install()
        .with_frozen()
        .with_skipped_with_deps(vec!["dummy-g".into()])
        .await
        .unwrap();
    let prefix = pixi.default_env_path().unwrap();
    // When filtering is active, the environment file should contain an invalid hash
    let env_file = prefix.join("conda-meta").join("pixi");
    let env_file_contents = fs_err::read_to_string(&env_file).expect("read environment file");
    assert!(
        env_file_contents.contains("invalid-hash"),
        "environment file should contain the invalid hash when filtering is active"
    );
    assert!(!is_conda_package_installed(&prefix, "dummy-g").await);
    assert!(!is_conda_package_installed(&prefix, "dummy-b").await);
    assert!(is_conda_package_installed(&prefix, "dummy-a").await);
}
