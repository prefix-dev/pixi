//! Integration tests for inline environment configuration.
//!
//! Tests verify that environments can define dependencies and other feature
//! configuration directly, without needing explicit feature definitions.

use pixi_manifest::{FeatureName, HasFeaturesIter, HasWorkspaceManifest, TaskName};
use pixi_test_utils::{MockRepoData, Package};
use rattler_conda_types::Platform;

use crate::common::{LockFileExt, PixiControl};
use crate::setup_tracing;

/// Test that inline environment dependencies are parsed and resolved correctly.
#[tokio::test]
async fn test_inline_environment_dependencies() {
    setup_tracing();

    let mut package_database = MockRepoData::default();
    package_database.add_package(Package::build("foo", "1").finish());

    let channel = package_database.into_channel().await.unwrap();
    let platform = Platform::current();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        name = "test-inline-env"
        channels = ["{channel_url}"]
        platforms = ["{platform}"]

        [environments.dev.dependencies]
        foo = "*"
        "#,
        channel_url = channel.url(),
    ))
    .unwrap();

    // Verify the workspace was created correctly
    let workspace = pixi.workspace().unwrap();
    let manifest = (&workspace).workspace_manifest();

    // Check that a synthetic feature was created with dot-prefixed key
    assert!(
        manifest.features.contains_key(&FeatureName::inline("dev")),
        "Synthetic feature '.dev' should be created"
    );

    // Check that the environment exists
    let env = workspace.environment("dev");
    assert!(env.is_some(), "Environment 'dev' should exist");

    // Verify the environment has the synthetic feature in its feature list
    let env = env.unwrap();
    assert!(
        env.features().any(|f| f.name == FeatureName::inline("dev")),
        "Environment should reference the synthetic '.dev' feature"
    );
}

/// Test inline environment with both explicit features and inline config.
#[tokio::test]
async fn test_inline_environment_with_explicit_features() {
    setup_tracing();

    let mut package_database = MockRepoData::default();
    package_database.add_package(Package::build("foo", "1").finish());
    package_database.add_package(Package::build("bar", "1").finish());

    let channel = package_database.into_channel().await.unwrap();
    let platform = Platform::current();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        name = "test-inline-with-features"
        channels = ["{channel_url}"]
        platforms = ["{platform}"]

        [feature.extra.dependencies]
        bar = "*"

        [environments.dev]
        features = ["extra"]

        [environments.dev.dependencies]
        foo = "*"
        "#,
        channel_url = channel.url(),
    ))
    .unwrap();

    let workspace = pixi.workspace().unwrap();
    let manifest = (&workspace).workspace_manifest();

    // Both features should exist (inline uses dot-prefix)
    assert!(manifest.features.contains_key(&FeatureName::from("extra")));
    assert!(manifest.features.contains_key(&FeatureName::inline("dev")));

    // The environment should have both features
    let env = workspace.environment("dev").unwrap();
    let feature_names: Vec<_> = env.features().map(|f| f.name.clone()).collect();

    assert!(
        feature_names.contains(&FeatureName::inline("dev")),
        "Environment should have synthetic '.dev' feature"
    );
    assert!(
        feature_names.contains(&FeatureName::from("extra")),
        "Environment should have explicit 'extra' feature"
    );
}

/// Test that inline environment tasks work correctly.
#[tokio::test]
async fn test_inline_environment_tasks() {
    setup_tracing();

    let mut package_database = MockRepoData::default();
    package_database.add_package(Package::build("foo", "1").finish());

    let channel = package_database.into_channel().await.unwrap();
    let platform = Platform::current();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        name = "test-inline-tasks"
        channels = ["{channel_url}"]
        platforms = ["{platform}"]

        [environments.dev.dependencies]
        foo = "*"

        [environments.dev.tasks]
        hello = "echo hello"
        "#,
        channel_url = channel.url(),
    ))
    .unwrap();

    let workspace = pixi.workspace().unwrap();
    let manifest = (&workspace).workspace_manifest();

    // Check that the synthetic feature has the task
    let dev_feature = manifest
        .features
        .get(&FeatureName::inline("dev"))
        .expect("dev feature should exist");

    assert!(
        dev_feature
            .targets
            .default()
            .tasks
            .contains_key(&TaskName::from("hello")),
        "Synthetic feature should have the 'hello' task"
    );
}

/// Test that lock file is generated correctly for inline environment.
#[tokio::test]
async fn test_inline_environment_lock_file() {
    setup_tracing();

    let mut package_database = MockRepoData::default();
    package_database.add_package(Package::build("foo", "1").finish());

    let channel = package_database.into_channel().await.unwrap();
    let platform = Platform::current();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        name = "test-inline-lock"
        channels = ["{channel_url}"]
        platforms = ["{platform}"]
        conda-pypi-map = {{}}

        [environments.dev.dependencies]
        foo = "*"
        "#,
        channel_url = channel.url(),
    ))
    .unwrap();

    // Update the lock file
    let lock = pixi.update_lock_file().await.unwrap();

    // Verify the environment exists in the lock file and has the dependency
    assert!(
        lock.contains_conda_package("dev", platform, "foo"),
        "Lock file should contain 'foo' for 'dev' environment"
    );
}
